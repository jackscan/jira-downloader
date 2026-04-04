#!/usr/bin/env bash
set -euo pipefail

MOCK_PORT=8080
MOCK_BASE_URL="http://127.0.0.1:${MOCK_PORT}"
ISSUE_KEY="${1:-PROJ-123}"

cleanup() {
    if [ -n "${MOCK_PID:-}" ]; then
        kill "$MOCK_PID" 2>/dev/null || true
        wait "$MOCK_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

echo "Starting mock Jira server on ${MOCK_BASE_URL}..."
cargo run --quiet --features mock-server --bin mock-server &
MOCK_PID=$!

echo "Waiting for mock server to be ready..."
for i in $(seq 1 30); do
    if curl -sf "${MOCK_BASE_URL}/health" >/dev/null 2>&1; then
        echo "Mock server is ready."
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "ERROR: Mock server did not start in time."
        exit 1
    fi
    sleep 0.5
done

echo ""
echo "Launching jira-downloader with issue ${ISSUE_KEY}..."
echo "Press Ctrl+C to exit."
echo ""

JIRA_BASE_URL="${MOCK_BASE_URL}" cargo run --bin jira-downloader -- "$ISSUE_KEY"
