#!/bin/bash

# Check if build was successful
if [ $? -ne 0 ]; then
    echo "Build failed. Exiting."
    exit 1
fi

/home/user/.cargo/bin/cargo build --release

# Request administrator privileges for the following operations
if [ "$EUID" -ne 0 ]; then
    echo "The following operations require administrator privileges. Please enter your password:"
    exec sudo -E "$0" "$@"
fi

# Enable IP forwarding
echo "Enabling IP forwarding..."
sysctl -w net.ipv4.ip_forward=1

# Make it persistent
if ! grep -q "net.ipv4.ip_forward=1" /etc/sysctl.conf; then
    echo "net.ipv4.ip_forward=1" >> /etc/sysctl.conf
    echo "Added to /etc/sysctl.conf for persistence"
fi

# Load nftables rules
echo "Loading nftables rules..."
nft -f nftables.conf

echo "Done!"
echo ""
echo "Current IP forwarding status:"
sysctl net.ipv4.ip_forward

echo ""
echo "Current nftables ruleset:"
nft list ruleset

# Create systemd service file
echo "Creating systemd service..."
SERVICE_FILE="/etc/systemd/system/adaptive-routing.service"
CURRENT_DIR=$(pwd)
BINARY_PATH="$CURRENT_DIR/target/release/adaptiverouting"

# Check if binary exists
if [ ! -f "$BINARY_PATH" ]; then
    echo "Error: Binary not found at $BINARY_PATH"
    exit 1
fi

tee $SERVICE_FILE > /dev/null << EOF
[Unit]
Description=Adaptive Routing Service
After=network.target

[Service]
Type=simple
ExecStart=$BINARY_PATH
WorkingDirectory=$CURRENT_DIR
Restart=always
RestartSec=5
User=root

[Install]
WantedBy=multi-user.target
EOF

# Reload systemd and enable the service
sudo systemctl daemon-reload
sudo systemctl enable adaptive-routing.service

echo "Service created and enabled. You can start it with:"
echo "sudo systemctl start adaptive-routing.service"