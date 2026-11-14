#!/bin/bash
set -e

echo "Starting ChaosNexus Anvil with chaosdata enabled..."
export CHAOSWRENCH_CONFIG="./anvil.toml"
./target/debug/anvil --name test_instance &
WRENCH_PID=$!

# Wait for it to boot
sleep 2

# Check if the process is still running
if kill -0 $WRENCH_PID 2>/dev/null; then
    echo "ChaosNexus Anvil started successfully!"
    kill $WRENCH_PID
    echo "Test passed."
else
    echo "ChaosNexus Anvil failed to start!"
    exit 1
fi
