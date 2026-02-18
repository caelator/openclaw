# KeyVault Orchestrator — Package

> Intelligent AI API key management + swarm routing for Antigravity

## What's Inside

```
keyvault-package/
├── bin/
│   └── keyvault           # Compiled daemon binary (macOS arm64)
├── skills/
│   ├── dual-model-orchestration/
│   │   ├── SKILL.md       # Architect + Coder dual-model protocol
│   │   └── templates/     # Code spec & report templates
│   └── iterative-plan-refinement/
│       └── SKILL.md       # Multi-model plan refinement skill
├── workflows/
│   └── dual-model.md      # /dual-model workflow trigger
├── install.sh             # One-click installer
└── README.md              # This file
```

## What It Does

**KeyVault** is a local daemon that manages AI API keys and provides intelligent routing:

- **Key Pool:** Stores encrypted API keys (AES-256-GCM + Argon2) in a local SQLite database
- **Swarm Scheduler:** Distributes tasks across 10+ API keys × 6 models automatically
- **Rate Tracking:** In-memory RPM/RPD tracking with automatic failover on 429s
- **Health Pulse:** 15-minute auto-monitoring with live utilization metrics
- **Complexity Classifier:** Routes tasks to the cheapest capable model (flash-lite → flash → pro)
- **6 LLM Adapters:** Google, Anthropic, OpenAI, Groq, DeepSeek, Perplexity

**Skills** are Antigravity extensions that teach the AI agent new capabilities:

- **Dual-Model Orchestration:** Splits work between an Architect (Claude Opus) and Coder (Gemini Pro)
- **Iterative Plan Refinement:** Passes plans through a model hierarchy for progressive improvement

## Quick Install

```bash
cd /path/to/your/project
./install.sh
```

The installer will:

1. Place the `keyvault` binary in `/usr/local/bin/`
2. Copy skills to `.agent/skills/` in your project
3. Copy workflows to `.agent/workflows/` in your project
4. Set up a `launchd` plist for auto-start on macOS
5. Generate a master encryption passphrase

## After Install

### 1. Start the daemon

```bash
launchctl load ~/Library/LaunchAgents/com.openclaw.keyvault.plist
```

### 2. Add API keys

```bash
# Get your auth token first
cat ~/.openclaw/auth_token

# Add keys via JSON-RPC (replace TOKEN and KEY_VALUE)
echo '{"jsonrpc":"2.0","method":"kv.admin.addKey","params":{"name":"google-1","value":"AIza...","provider":"google"},"id":1}' \
  | socat - UNIX-CONNECT:~/.openclaw/keyvault.sock
```

### 3. Verify

```bash
echo '{"jsonrpc":"2.0","method":"kv.health","id":1}' \
  | socat - UNIX-CONNECT:~/.openclaw/keyvault.sock
```

### 4. Use in Antigravity

Type `/dual-model` in any Antigravity conversation to activate dual-model orchestration.

## API Endpoints

| Method                 | Auth | Description                           |
| ---------------------- | ---- | ------------------------------------- |
| `kv.health`            | No   | Key health + live pulse metrics       |
| `kv.models`            | No   | Available models from all providers   |
| `kv.activeModels`      | No   | Currently usable model list           |
| `kv.modelRegistry`     | No   | Swarm model registry with specs       |
| `kv.swarmStatus`       | No   | Per-key RPM/RPD utilization dashboard |
| `kv.generate`          | Yes  | Single-key generation request         |
| `kv.parallelGenerate`  | Yes  | Fan-out across N keys                 |
| `kv.swarmGenerate`     | Yes  | Auto-classify + route + failover      |
| `kv.admin.addKey`      | Yes  | Add an encrypted API key              |
| `kv.admin.removeKey`   | Yes  | Remove a key                          |
| `kv.admin.listKeys`    | Yes  | List all stored keys                  |
| `kv.admin.rotateToken` | Yes  | Rotate the auth token                 |

## Requirements

- **macOS** (arm64 or Intel)
- **socat** for testing (`brew install socat`)
- API keys from at least one supported provider

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                   Antigravity                       │
│                                                     │
│  ┌──────────────┐  ┌────────────────────────────┐   │
│  │ /dual-model  │  │  iterative-plan-refinement │   │
│  │   workflow   │  │          skill             │   │
│  └──────┬───────┘  └────────────────────────────┘   │
│         │                                           │
│         ▼                                           │
│  ┌──────────────────────────────────────────────┐   │
│  │    dual-model-orchestration SKILL.md         │   │
│  │    (Architect ↔ Coder protocol)              │   │
│  └──────────────────────────────────────────────┘   │
└────────────────────────┬────────────────────────────┘
                         │ JSON-RPC over Unix Socket
                         ▼
┌─────────────────────────────────────────────────────┐
│              KeyVault Daemon                        │
│                                                     │
│  ┌──────────┐ ┌──────────┐ ┌────────────────────┐   │
│  │  Server  │ │  Auth    │ │   Rate Limiter     │   │
│  └────┬─────┘ └──────────┘ └────────────────────┘   │
│       │                                             │
│  ┌────▼─────────────────────────────────────────┐   │
│  │            Pool Manager                      │   │
│  │  ┌────────────┐ ┌───────────┐ ┌──────────┐   │   │
│  │  │ Classifier │ │ Swarm     │ │ Rate     │   │   │
│  │  │            │ │ Scheduler │ │ Tracker  │   │   │
│  │  └────────────┘ └───────────┘ └──────────┘   │   │
│  └──────────────────────────────────────────────┘   │
│       │                                             │
│  ┌────▼─────────────────────────────────────────┐   │
│  │  Adapters: Google│Anthropic│OpenAI│Groq│...  │   │
│  └──────────────────────────────────────────────┘   │
│       │                                             │
│  ┌────▼─────────────────────────────────────────┐   │
│  │  Vault (AES-256-GCM + SQLite)               │   │
│  └──────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────┘
```
