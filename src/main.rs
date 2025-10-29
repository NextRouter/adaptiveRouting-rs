use anyhow::{bail, Context, Result};
use axum::{extract::Query, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::env;
use std::process::Command;
use std::sync::Arc;
use tokio::sync::Mutex;

mod version {
    pub const VERSION: &str = "1.0.0";
}

#[derive(Clone)]
struct Config {
    wan0: String,
    wan1: String,
    lan: String,
}

impl Config {
    fn from_env() -> Self {
        Config {
            wan0: env::var("WAN0").unwrap_or_else(|_| "eth0".to_string()),
            wan1: env::var("WAN1").unwrap_or_else(|_| "eth1".to_string()),
            lan: env::var("LAN").unwrap_or_else(|_| "eth2".to_string()),
        }
    }
}

#[derive(Clone)]
struct AppState {
    mappings: Arc<Mutex<std::collections::HashMap<String, String>>>,
    config: Config,
}

#[derive(Deserialize)]
struct SwitchParams {
    ip: String,
    nic: String,
}

#[derive(Serialize)]
struct ApiResponse {
    status: String,
    message: String,
}

fn run_cmd(cmd: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {} {:?}", cmd, args))?;
    if !out.status.success() {
        bail!(
            "{} {:?} failed: {}",
            cmd,
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

// ---- Policy routing helpers ----

const TABLE_WAN0: &str = "100"; // routing table id for wan0
const TABLE_WAN1: &str = "200"; // routing table id for wan1
const PRIO_SPECIFIC: &str = "1000"; // higher priority (smaller number)
const PRIO_LAN_DEFAULT: &str = "2000"; // default lan policy priority

fn get_default_gateway_for_iface(iface: &str) -> Result<String> {
    // Try to read default route for specific iface
    let out = run_cmd("ip", &["route", "show", "default", "dev", iface])?;
    let re = Regex::new(r"via\s+(\d+\.\d+\.\d+\.\d+)").expect("regex compiles");
    if let Some(cap) = re.captures(&out) {
        return Ok(cap[1].to_string());
    }
    // Fallback: scan all defaults and pick the one matching iface
    let all = run_cmd("ip", &["route", "show", "default"])?
        .lines()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    for line in all.lines() {
        if line.contains(&format!(" dev {}", iface)) {
            if let Some(cap) = re.captures(line) {
                return Ok(cap[1].to_string());
            }
        }
    }
    bail!("Could not determine default gateway for iface {}", iface)
}

fn ensure_table_default_route(iface: &str, table: &str, gw: &str) -> Result<()> {
    // Create/replace default route for table
    run_cmd(
        "ip",
        &[
            "route", "replace", "default", "via", gw, "dev", iface, "table", table,
        ],
    )?;
    Ok(())
}

fn ip_rule_list() -> Result<String> {
    run_cmd("ip", &["rule", "show"])
}

fn ip_rule_exists(from: &str, table: &str) -> Result<bool> {
    let rules = ip_rule_list()?;
    let needle = format!("from {} lookup {}", from, table);
    Ok(rules.lines().any(|l| l.contains(&needle)))
}

fn add_ip_rule(from: &str, table: &str, prio: &str) -> Result<()> {
    if !ip_rule_exists(from, table)? {
        run_cmd(
            "ip",
            &[
                "rule", "add", "from", from, "lookup", table, "priority", prio,
            ],
        )?;
    }
    Ok(())
}

fn del_ip_rule_quiet(from: &str, table: &str) {
    // Best-effort delete; ignore errors
    let _ = Command::new("ip")
        .args(["rule", "del", "from", from, "lookup", table])
        .output();
}

fn mirror_link_routes_to_table(iface: &str, table: &str) -> Result<()> {
    // Copy "scope link" routes of the interface into the given table
    let out = run_cmd(
        "ip",
        &["-4", "route", "show", "dev", iface, "scope", "link"],
    )?;
    let re = Regex::new(r"^(\d+\.\d+\.\d+\.\d+(?:/\d+)?)\b").expect("regex compiles");
    for line in out.lines() {
        if let Some(cap) = re.captures(line) {
            let prefix = &cap[1];
            // Replace/ensure route exists in the custom table
            let _ = run_cmd(
                "ip",
                &[
                    "route", "replace", prefix, "dev", iface, "scope", "link", "table", table,
                ],
            );
        }
    }
    Ok(())
}

async fn switch_handler(
    Query(params): Query<SwitchParams>,
    state: axum::extract::State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if params.nic != "wan0" && params.nic != "wan1" {
        return Err((
            StatusCode::BAD_REQUEST,
            "nic must be 'wan0' or 'wan1'".to_string(),
        ));
    }

    // Parse IP address - expecting format like "10.40.0.3/20"
    let ip_re = Regex::new(r"^(\d+\.\d+\.\d+\.\d+)(/\d+)?$").unwrap();
    let caps = ip_re.captures(&params.ip).ok_or((
        StatusCode::BAD_REQUEST,
        "Invalid IP format. Expected: IP or IP/subnet (e.g., 10.40.0.3 or 10.40.0.3/20)"
            .to_string(),
    ))?;

    let base_ip = &caps[1];

    // Ensure we use /32 (single host) for the actual IP command
    let target_ip = format!("{}/32", base_ip);

    // Policy routing approach:
    // - Default: entire 10.40.0.0/20 goes to wan0 via routing table 100
    // - Override: specific /32 can be forced to wan1 via table 200

    // First, clear any existing per-IP rules for both tables
    del_ip_rule_quiet(&target_ip, TABLE_WAN0);
    del_ip_rule_quiet(&target_ip, TABLE_WAN1);

    let message: String;
    if params.nic == "wan1" {
        // Add specific rule to wan1
        if let Err(e) = add_ip_rule(&target_ip, TABLE_WAN1, PRIO_SPECIFIC) {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to add policy rule: {}", e),
            ));
        }
        message = format!(
            "Routed {} to wan1 ({}) via policy",
            target_ip, state.config.wan1
        );
    } else {
        // For wan0, we rely on the default LAN rule; no per-IP rule needed
        message = format!(
            "Routed {} to wan0 ({}) via default policy",
            target_ip, state.config.wan0
        );
    }

    let mut mappings = state.mappings.lock().await;
    mappings.insert(base_ip.to_string(), params.nic.clone());

    let response = ApiResponse {
        status: "success".to_string(),
        message,
    };

    Ok((StatusCode::OK, Json(response)))
}

async fn status_handler(state: axum::extract::State<AppState>) -> impl IntoResponse {
    let mappings = state.mappings.lock().await;
    Json(serde_json::json!({
        "mappings": mappings.clone(),
        "config": {
            "wan0": state.config.wan0,
            "wan1": state.config.wan1,
            "lan": state.config.lan
        }
    }))
}

async fn initialize_lan_to_wan0(config: &Config) -> Result<()> {
    // Establish policy routing so that 10.40.0.0/20 goes out via wan0 by default
    let lan_subnet = "10.40.0.0/20";

    println!(
        "Initializing policy routing: {} -> wan0 ({})",
        lan_subnet, config.wan0
    );

    // Clean up any previous incorrect address assignments on WAN interfaces (best-effort)
    let _ = Command::new("ip")
        .args(["addr", "del", lan_subnet, "dev", &config.wan0])
        .output();
    let _ = Command::new("ip")
        .args(["addr", "del", lan_subnet, "dev", &config.wan1])
        .output();

    // Discover gateways
    let gw0 = get_default_gateway_for_iface(&config.wan0)
        .with_context(|| format!("get gateway for {}", &config.wan0))?;
    let gw1 = get_default_gateway_for_iface(&config.wan1)
        .with_context(|| format!("get gateway for {}", &config.wan1))?;

    // Ensure routing tables have default routes
    ensure_table_default_route(&config.wan0, TABLE_WAN0, &gw0)
        .with_context(|| format!("set table {} default route", TABLE_WAN0))?;
    ensure_table_default_route(&config.wan1, TABLE_WAN1, &gw1)
        .with_context(|| format!("set table {} default route", TABLE_WAN1))?;

    // Also mirror directly-connected link routes into each table (for ARP/gw resolution)
    mirror_link_routes_to_table(&config.wan0, TABLE_WAN0).with_context(|| {
        format!(
            "mirror link routes for {} to table {}",
            &config.wan0, TABLE_WAN0
        )
    })?;
    mirror_link_routes_to_table(&config.wan1, TABLE_WAN1).with_context(|| {
        format!(
            "mirror link routes for {} to table {}",
            &config.wan1, TABLE_WAN1
        )
    })?;

    // Ensure base rule for LAN subnet -> wan0 table
    add_ip_rule(lan_subnet, TABLE_WAN0, PRIO_LAN_DEFAULT)
        .with_context(|| "add base LAN policy rule".to_string())?;

    println!(
        "Policy ready: {} uses table {}, specific hosts can be overridden to table {}",
        lan_subnet, TABLE_WAN0, TABLE_WAN1
    );
    Ok(())
}

#[tokio::main]
async fn main() {
    let config = Config::from_env();
    println!("Configuration:");
    println!("  wan0: {}", config.wan0);
    println!("  wan1: {}", config.wan1);
    println!("  lan: {}", config.lan);

    if let Err(e) = initialize_lan_to_wan0(&config).await {
        eprintln!("Failed to initialize: {}", e);
        std::process::exit(1);
    }

    let state = AppState {
        mappings: Arc::new(Mutex::new(std::collections::HashMap::new())),
        config,
    };

    let app = Router::new()
        .route("/switch", get(switch_handler))
        .route("/status", get(status_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:32599")
        .await
        .expect("Failed to bind to port 32599");

    println!(
        "Server listening on http://127.0.0.1:32599 => {}",
        version::VERSION
    );

    axum::serve(listener, app).await.expect("Server error");
}
