# cosmic-applet-agents-status

Native COSMIC desktop applet for monitoring coding agent status.

## Features

- Real-time status display in the panel
- Hover popup with detailed information
- Monitors multiple coding agents:
  - **Claude Code**: OAuth usage (current/weekly utilization)
  - **OpenRouter**: Credit balance and usage
  - **Codex CLI**: Login status and session count
  - **Gemini CLI**: Account status
  - **GitHub Copilot**: Login status and sessions
  - Generic detection for other agents

## Installation

### Nix Flake

```nix
# In your flake inputs
inputs.cosmic-applet-agents-status.url = "github:deepwatrcreatur/cosmic-applet-agents-status";

# In home.packages
inputs.cosmic-applet-agents-status.packages.${system}.default
```

### Desktop File

Add a desktop file to `~/.local/share/applications/`:

```ini
[Desktop Entry]
Name=Agents Status
Type=Application
Exec=cosmic-applet-agents-status
Terminal=false
Categories=COSMIC;
Icon=utilities-terminal-symbolic
NoDisplay=true
X-CosmicApplet=true
X-CosmicHoverPopup=Auto
```

## Configuration

Create `~/.config/cosmic-applet-agents-status/config.toml`:

```toml
poll_seconds = 90
claude_cache_ttl_seconds = 60

# Optional: Path to OpenRouter API key (for usage tracking)
openrouter_api_key_path = "/run/agenix/openrouter-api-key"

[[agents]]
id = "claude"
name = "Claude Code"
command = "claude"

[[agents]]
id = "openrouter"
name = "OpenRouter"
command = "true"  # Always "installed" - usage comes from API

[[agents]]
id = "codex"
name = "Codex CLI"
command = "codex"

[[agents]]
id = "gemini"
name = "Gemini CLI"
command = "gemini"

[[agents]]
id = "copilot"
name = "GitHub Copilot"
command = "copilot"
```

### Config Options

| Option | Description | Default |
|--------|-------------|---------|
| `poll_seconds` | Refresh interval in seconds (minimum 5) | `90` |
| `claude_cache_ttl_seconds` | Cache duration for Claude API responses | `60` |
| `openrouter_api_key_path` | Path to OpenRouter API key file | `/run/agenix/openrouter-api-key` |

### Agent Definition

| Field | Description |
|-------|-------------|
| `id` | Agent identifier (claude, openrouter, codex, gemini, copilot, or custom) |
| `name` | Display name shown in the applet |
| `command` | Command used to check if agent is installed |

## OpenRouter API Key

To enable OpenRouter usage tracking:

1. Get your API key from [OpenRouter](https://openrouter.ai/keys)

2. Store the key securely (recommended: use agenix or sops-nix):
   ```bash
   # With agenix
   echo "sk-or-v1-..." | rage -e -r "ssh-ed25519 AAAA..." -o secrets-agenix/openrouter-api-key.age
   ```

3. Configure agenix to decrypt at `/run/agenix/openrouter-api-key`

4. Set `openrouter_api_key_path` in config.toml

The applet queries OpenRouter's `/api/v1/auth/key` endpoint to display:
- Credit usage and remaining balance
- Rate limit information
- Account tier (free/paid)

## Adding to COSMIC Panel

1. Open **COSMIC Settings > Desktop > Panel**
2. Click on your panel
3. Go to **Applets** section
4. Find "Agents Status" and add it

## License

MIT
