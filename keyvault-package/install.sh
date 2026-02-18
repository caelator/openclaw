#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# KeyVault Orchestrator — Installer for Antigravity
# =============================================================================
#
# This script installs the KeyVault daemon and its Antigravity skills into
# any project that uses Antigravity (Claude's agentic coding tool).
#
# What gets installed:
#   1. keyvault binary         → /usr/local/bin/keyvault
#   2. Skills                  → <project>/.agent/skills/
#   3. Workflows               → <project>/.agent/workflows/
#   4. launchd plist           → ~/Library/LaunchAgents/ (macOS auto-start)
#
# Usage:
#   cd /path/to/your/project
#   ./install.sh
#
# Prerequisites:
#   - macOS (arm64 or x86_64)
#   - A project directory with .agent/ (or it will be created)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="${1:-$(pwd)}"

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║          KeyVault Orchestrator — Installer v1.0             ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""
echo "  Package dir:  $SCRIPT_DIR"
echo "  Project dir:  $PROJECT_DIR"
echo ""

# ── 1. Install binary ──────────────────────────────────────────────
echo "━━━ Step 1/4: Installing keyvault binary ━━━"

BINARY="$SCRIPT_DIR/bin/keyvault"
if [[ ! -f "$BINARY" ]]; then
    echo "  ✗ Binary not found at $BINARY"
    echo "  → Build it first: cargo build --release -p keyvault"
    exit 1
fi

INSTALL_DIR="/usr/local/bin"
echo "  Installing to $INSTALL_DIR/keyvault..."
if [[ -w "$INSTALL_DIR" ]]; then
    cp "$BINARY" "$INSTALL_DIR/keyvault"
else
    echo "  (requires sudo)"
    sudo cp "$BINARY" "$INSTALL_DIR/keyvault"
fi
chmod +x "$INSTALL_DIR/keyvault"
echo "  ✓ Binary installed"

# ── 2. Install skills ──────────────────────────────────────────────
echo ""
echo "━━━ Step 2/4: Installing Antigravity skills ━━━"

AGENT_DIR="$PROJECT_DIR/.agent"
mkdir -p "$AGENT_DIR/skills" "$AGENT_DIR/workflows"

# Dual-model orchestration skill
SKILL_DST="$AGENT_DIR/skills/dual-model-orchestration"
mkdir -p "$SKILL_DST/templates"
cp "$SCRIPT_DIR/skills/dual-model-orchestration/SKILL.md" "$SKILL_DST/"
cp "$SCRIPT_DIR/skills/dual-model-orchestration/templates/"*.md "$SKILL_DST/templates/"
echo "  ✓ Installed skill: dual-model-orchestration"

# Iterative plan refinement skill
SKILL_DST2="$AGENT_DIR/skills/iterative-plan-refinement"
mkdir -p "$SKILL_DST2"
cp "$SCRIPT_DIR/skills/iterative-plan-refinement/SKILL.md" "$SKILL_DST2/"
echo "  ✓ Installed skill: iterative-plan-refinement"

# ── 3. Install workflows ──────────────────────────────────────────
echo ""
echo "━━━ Step 3/4: Installing Antigravity workflows ━━━"

cp "$SCRIPT_DIR/workflows/dual-model.md" "$AGENT_DIR/workflows/"
echo "  ✓ Installed workflow: /dual-model"

# ── 4. Set up launchd (macOS daemon) ──────────────────────────────
echo ""
echo "━━━ Step 4/4: Setting up KeyVault daemon ━━━"

PLIST_DIR="$HOME/Library/LaunchAgents"
PLIST_FILE="$PLIST_DIR/com.openclaw.keyvault.plist"
SOCKET_DIR="$HOME/.openclaw"
mkdir -p "$SOCKET_DIR" "$PLIST_DIR"

# Generate master passphrase if not set
if [[ ! -f "$SOCKET_DIR/.master_passphrase" ]]; then
    echo "  Generating master passphrase..."
    PASSPHRASE=$(openssl rand -base64 32)
    echo "$PASSPHRASE" > "$SOCKET_DIR/.master_passphrase"
    chmod 600 "$SOCKET_DIR/.master_passphrase"
    echo "  ✓ Master passphrase saved to $SOCKET_DIR/.master_passphrase"
else
    echo "  ✓ Master passphrase already exists"
fi

cat > "$PLIST_FILE" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.openclaw.keyvault</string>

    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/keyvault</string>
        <string>serve</string>
    </array>

    <key>EnvironmentVariables</key>
    <dict>
        <key>KEYVAULT_PASSPHRASE_FILE</key>
        <string>${SOCKET_DIR}/.master_passphrase</string>
    </dict>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>

    <key>StandardOutPath</key>
    <string>${SOCKET_DIR}/keyvault.log</string>
    <key>StandardErrorPath</key>
    <string>${SOCKET_DIR}/keyvault.err</string>

    <key>SoftResourceLimits</key>
    <dict>
        <key>NumberOfFiles</key>
        <integer>1024</integer>
    </dict>
</dict>
</plist>
PLIST

echo "  ✓ Created launchd plist: $PLIST_FILE"
echo ""

# ── Summary ───────────────────────────────────────────────────────
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║                    Installation Complete                    ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""
echo "  Binary:    /usr/local/bin/keyvault"
echo "  Skills:    $AGENT_DIR/skills/"
echo "  Workflows: $AGENT_DIR/workflows/"
echo "  Daemon:    $PLIST_FILE"
echo "  Logs:      $SOCKET_DIR/keyvault.log"
echo ""
echo "━━━ Next Steps ━━━"
echo ""
echo "  1. Start the daemon:"
echo "     launchctl load $PLIST_FILE"
echo ""
echo "  2. Add your API keys:"
echo "     keyvault add-key google-1 --provider google"
echo "     (repeat for each key)"
echo ""
echo "  3. Verify it's running:"
echo "     echo '{\"jsonrpc\":\"2.0\",\"method\":\"kv.health\",\"id\":1}' | socat - UNIX-CONNECT:$SOCKET_DIR/keyvault.sock"
echo ""
echo "  4. Use in Antigravity:"
echo "     Type '/dual-model' to activate the dual-model orchestration skill"
echo ""
