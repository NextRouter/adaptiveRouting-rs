#!/bin/bash

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
