#!/bin/bash
# Install / uninstall rhythm-server as a system service
#
# Linux (system):  sudo ./install/install.sh
# Linux (user):    ./install/install.sh --user
# macOS:           ./install/install.sh
# Uninstall:       ./install/install.sh --uninstall

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Defaults
BINARY=""
USER_MODE=false
UNINSTALL=false
PORT=""
LOG_LEVEL=""
NO_START="${RHYTHM_NO_START:-}"
[ "${CI:-}" = "true" ] && NO_START=1

PLIST_LABEL="com.rhythm.lighting.server"
SERVICE_NAME="rhythm-server"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --binary)
            BINARY="$2"
            shift 2
            ;;
        --user)
            USER_MODE=true
            shift
            ;;
        --port)
            PORT="$2"
            shift 2
            ;;
        --log-level)
            LOG_LEVEL="$2"
            shift 2
            ;;
        --uninstall)
            UNINSTALL=true
            shift
            ;;
        --no-start)
            NO_START=1
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Install rhythm-server as a system service (auto-start, auto-restart)."
            echo ""
            echo "Options:"
            echo "  --binary PATH       Path to pre-built binary (default: auto-detect from dist/bin/)"
            echo "  --user              Linux: install as user-level systemd service (no root)"
            echo "  --port PORT         Override default port (54448)"
            echo "  --log-level LEVEL   Override default log level (info)"
            echo "  --uninstall         Remove service, binary (prompts before removing data)"
            echo "  --no-start          Install but do not start the service (also via RHYTHM_NO_START=1 / CI=true)"
            echo "  -h, --help          Show this help"
            echo ""
            echo "Platforms:"
            echo "  Linux (root)        systemd system service, /usr/local/bin, /var/lib/rhythm"
            echo "  Linux (--user)      systemd user service, ~/.local/bin, ~/.rhythm"
            echo "  macOS               launchd LaunchAgent, /usr/local/bin, ~/.rhythm"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

OS="$(uname -s)"

# --- Helper functions ---

auto_detect_binary() {
    if [ -n "$BINARY" ]; then
        if [ ! -f "$BINARY" ]; then
            echo "Error: Binary not found at $BINARY"
            exit 1
        fi
        return
    fi

    # Determine expected output dir
    local os_name arch
    case "$OS" in
        Darwin) os_name="macos" ;;
        Linux)  os_name="linux" ;;
        *)      echo "Error: Unsupported OS: $OS"; exit 1 ;;
    esac
    case "$(uname -m)" in
        x86_64)         arch="x86_64" ; [ "$os_name" = "linux" ] && arch="amd64" ;;
        aarch64|arm64)  arch="arm64" ; [ "$os_name" = "linux" ] && arch="aarch64" ;;
        armv6l|arm1176*) arch="armv6l" ; [ "$os_name" = "linux" ] && arch="rpiz" ;;
        *)              arch="$(uname -m)" ;;
    esac

    local candidate="$PROJECT_ROOT/dist/bin/${os_name}-${arch}/rhythm-server"
    if [ -f "$candidate" ]; then
        BINARY="$candidate"
        echo "Found binary: $BINARY"
        return
    fi

    # Fallback: check legacy path
    local legacy_arch
    case "$(uname -m)" in
        x86_64)         legacy_arch="amd64" ;;
        aarch64|arm64)  legacy_arch="aarch64" ;;
        armv6l|arm1176*) legacy_arch="rpiz" ;;
        *)              legacy_arch="$(uname -m)" ;;
    esac
    candidate="$PROJECT_ROOT/dist/bin/${legacy_arch}/rhythm-server"
    if [ -f "$candidate" ]; then
        BINARY="$candidate"
        echo "Found binary (legacy path): $BINARY"
        return
    fi

    echo "Error: No binary found. Build first with: ./scripts/build-server.sh --release"
    exit 1
}

build_extra_args() {
    local args=""
    if [ -n "$PORT" ]; then
        args="$args --port $PORT"
    fi
    if [ -n "$LOG_LEVEL" ]; then
        args="$args --log-level $LOG_LEVEL"
    fi
    echo "$args"
}

confirm_remove_data() {
    local data_dir="$1"
    if [ ! -d "$data_dir" ]; then
        return
    fi
    echo ""
    echo "Data directory exists: $data_dir"
    REPLY=""
    if [ -t 0 ]; then
        read -p "Remove data directory? This cannot be undone. [y/N] " -r REPLY || REPLY=""
    else
        echo "(non-interactive: keeping data)"
    fi
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        rm -rf "$data_dir"
        echo "Removed $data_dir"
    else
        echo "Kept $data_dir"
    fi
}

install_binaries() {
    local bin_dir="$1"
    local need_sudo="$2"
    local cli_binary
    local chipd_binary
    cli_binary="$(dirname "$BINARY")/rhythm-cli"
    chipd_binary="$(dirname "$BINARY")/rhythm-chipd"

    echo "Installing binaries to $bin_dir/..."
    if [ "$need_sudo" = true ]; then
        sudo cp "$BINARY" "$bin_dir/rhythm-server"
        if [ -f "$cli_binary" ]; then
            sudo cp "$cli_binary" "$bin_dir/rhythm-cli"
        fi
        if [ -f "$chipd_binary" ]; then
            sudo cp "$chipd_binary" "$bin_dir/rhythm-chipd"
        fi
    else
        cp "$BINARY" "$bin_dir/rhythm-server"
        if [ -f "$cli_binary" ]; then
            cp "$cli_binary" "$bin_dir/rhythm-cli"
        fi
        if [ -f "$chipd_binary" ]; then
            cp "$chipd_binary" "$bin_dir/rhythm-chipd"
        fi
    fi
    chmod +x "$bin_dir/rhythm-server"
    if [ -f "$bin_dir/rhythm-cli" ]; then
        chmod +x "$bin_dir/rhythm-cli"
    fi
    if [ -f "$bin_dir/rhythm-chipd" ]; then
        chmod +x "$bin_dir/rhythm-chipd"
    fi
}

# --- macOS (launchd) ---

macos_install() {
    auto_detect_binary

    local bin_dir="/usr/local/bin"
    local data_dir="$HOME/.rhythm"
    local log_dir="$HOME/Library/Logs/Rhythm"
    local plist_dir="$HOME/Library/LaunchAgents"
    local plist_path="$plist_dir/$PLIST_LABEL.plist"
    local extra_args
    extra_args=$(build_extra_args)

    # Stop existing service if running
    if launchctl list "$PLIST_LABEL" &>/dev/null; then
        echo "Stopping existing service..."
        launchctl bootout "gui/$(id -u)/$PLIST_LABEL" 2>/dev/null || true
    fi

    # Copy binaries
    local need_sudo=false
    [ ! -w "$bin_dir" ] && need_sudo=true
    install_binaries "$bin_dir" "$need_sudo"

    # Create directories
    mkdir -p "$data_dir" "$log_dir" "$plist_dir"

    # Generate plist from template
    local plist_content
    plist_content=$(<"$SCRIPT_DIR/macos/com.rhythm.lighting.server.plist")
    plist_content="${plist_content//__DATA_DIR__/$data_dir}"
    plist_content="${plist_content//__LOG_DIR__/$log_dir}"

    # Inject extra args into ProgramArguments
    if [ -n "$PORT" ]; then
        plist_content="${plist_content//<string>--log-level<\/string>/<string>--port<\/string>\n        <string>$PORT<\/string>\n        <string>--log-level<\/string>}"
    fi
    if [ -n "$LOG_LEVEL" ]; then
        plist_content="${plist_content//<string>info<\/string>/<string>$LOG_LEVEL<\/string>}"
    fi

    echo "$plist_content" > "$plist_path"

    if [ -n "$NO_START" ]; then
        echo "Installed plist (not starting; --no-start)."
    else
        # Load service
        echo "Loading service..."
        launchctl bootstrap "gui/$(id -u)" "$plist_path"
    fi

    echo ""
    echo "Installed successfully!"
    echo "  Server:  $bin_dir/rhythm-server"
    echo "  CLI:     $bin_dir/rhythm-cli"
    echo "  Data:    $data_dir"
    echo "  Logs:    $log_dir"
    echo "  Plist:   $plist_path"
    echo ""
    echo "Commands:"
    echo "  rhythm-cli status"
    echo "  rhythm-cli stop"
    echo "  rhythm-cli start"
    echo "  rhythm-cli update"
    echo "  rhythm-cli uninstall"
    echo "  tail -f $log_dir/rhythm-server.log"
}

macos_uninstall() {
    local bin_dir="/usr/local/bin"
    local bin_path="$bin_dir/rhythm-server"
    local cli_path="$bin_dir/rhythm-cli"
    local chipd_path="$bin_dir/rhythm-chipd"
    local data_dir="$HOME/.rhythm"
    local log_dir="$HOME/Library/Logs/Rhythm"
    local plist_path="$HOME/Library/LaunchAgents/$PLIST_LABEL.plist"

    # Stop and unload
    if launchctl list "$PLIST_LABEL" &>/dev/null; then
        echo "Stopping service..."
        launchctl bootout "gui/$(id -u)/$PLIST_LABEL" 2>/dev/null || true
    fi

    # Remove plist
    if [ -f "$plist_path" ]; then
        rm "$plist_path"
        echo "Removed $plist_path"
    fi

    # Remove binaries
    for bin in "$bin_path" "$cli_path" "$chipd_path"; do
        if [ -f "$bin" ]; then
            if [ -w "$bin" ]; then
                rm "$bin"
            else
                sudo rm "$bin"
            fi
            echo "Removed $bin"
        fi
    done

    # Remove logs
    if [ -d "$log_dir" ]; then
        rm -rf "$log_dir"
        echo "Removed $log_dir"
    fi

    confirm_remove_data "$data_dir"

    echo ""
    echo "rhythm-server uninstalled."
}

# --- Linux system (systemd) ---

linux_system_install() {
    if [ "$(id -u)" -ne 0 ]; then
        echo "Error: System install requires root. Use sudo or pass --user for user-level install."
        exit 1
    fi

    auto_detect_binary

    local bin_dir="/usr/local/bin"
    local data_dir="/var/lib/rhythm"
    local service_path="/etc/systemd/system/$SERVICE_NAME.service"
    local extra_args
    extra_args=$(build_extra_args)

    # Stop existing service
    if systemctl is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
        echo "Stopping existing service..."
        systemctl stop "$SERVICE_NAME"
    fi

    # Create rhythm user/group
    if ! id -u rhythm &>/dev/null; then
        echo "Creating rhythm system user..."
        useradd --system --no-create-home --shell /usr/sbin/nologin rhythm
    fi

    # Copy binaries
    install_binaries "$bin_dir" false

    # Create data dir
    mkdir -p "$data_dir"
    chown rhythm:rhythm "$data_dir"

    # Build ExecStart line
    local exec_start="$bin_dir/rhythm-server --data-dir $data_dir --log-level ${LOG_LEVEL:-info}"
    if [ -n "$PORT" ]; then
        exec_start="$exec_start --port $PORT"
    fi

    # Generate unit file from template, replacing ExecStart
    local unit_content
    unit_content=$(<"$SCRIPT_DIR/linux/rhythm-server.service")
    # Replace the ExecStart line
    unit_content=$(echo "$unit_content" | sed "s|^ExecStart=.*|ExecStart=$exec_start|")

    echo "$unit_content" > "$service_path"

    # Enable and start
    systemctl daemon-reload
    systemctl enable "$SERVICE_NAME"
    if [ -n "$NO_START" ]; then
        echo "Enabled but not started (--no-start)."
    else
        systemctl start "$SERVICE_NAME"
    fi

    echo ""
    echo "Installed successfully!"
    echo "  Server:   $bin_dir/rhythm-server"
    echo "  CLI:      $bin_dir/rhythm-cli"
    echo "  Data:     $data_dir"
    echo "  Service:  $service_path"
    echo ""
    echo "Commands:"
    echo "  rhythm-cli status"
    echo "  rhythm-cli stop"
    echo "  rhythm-cli start"
    echo "  rhythm-cli update"
    echo "  rhythm-cli uninstall"
    echo "  journalctl -u $SERVICE_NAME -f"
}

linux_system_uninstall() {
    if [ "$(id -u)" -ne 0 ]; then
        echo "Error: System uninstall requires root."
        exit 1
    fi

    local bin_dir="/usr/local/bin"
    local bin_path="$bin_dir/rhythm-server"
    local cli_path="$bin_dir/rhythm-cli"
    local chipd_path="$bin_dir/rhythm-chipd"
    local data_dir="/var/lib/rhythm"
    local service_path="/etc/systemd/system/$SERVICE_NAME.service"

    # Stop and disable
    if systemctl is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
        echo "Stopping service..."
        systemctl stop "$SERVICE_NAME"
    fi
    if systemctl is-enabled --quiet "$SERVICE_NAME" 2>/dev/null; then
        systemctl disable "$SERVICE_NAME"
    fi

    # Remove service file
    if [ -f "$service_path" ]; then
        rm "$service_path"
        systemctl daemon-reload
        echo "Removed $service_path"
    fi

    # Remove binaries
    for bin in "$bin_path" "$cli_path" "$chipd_path"; do
        if [ -f "$bin" ]; then
            rm "$bin"
            echo "Removed $bin"
        fi
    done

    confirm_remove_data "$data_dir"

    echo ""
    echo "rhythm-server uninstalled."
}

# --- Linux user (systemd --user) ---

linux_user_install() {
    auto_detect_binary

    local bin_dir="$HOME/.local/bin"
    local data_dir="$HOME/.rhythm"
    local service_dir="$HOME/.config/systemd/user"
    local service_path="$service_dir/$SERVICE_NAME.service"
    local extra_args
    extra_args=$(build_extra_args)

    # Stop existing service
    if systemctl --user is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
        echo "Stopping existing service..."
        systemctl --user stop "$SERVICE_NAME"
    fi

    # Copy binaries
    mkdir -p "$bin_dir"
    install_binaries "$bin_dir" false

    # Create data dir
    mkdir -p "$data_dir"

    # Build ExecStart line
    local exec_start="$bin_dir/rhythm-server --data-dir $data_dir --log-level ${LOG_LEVEL:-info}"
    if [ -n "$PORT" ]; then
        exec_start="$exec_start --port $PORT"
    fi

    # Generate user unit file
    mkdir -p "$service_dir"
    cat > "$service_path" <<EOF
[Unit]
Description=Rhythm OS Server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$exec_start
Restart=on-failure
RestartSec=5
StartLimitIntervalSec=60
StartLimitBurst=5
StandardOutput=journal
StandardError=journal
SyslogIdentifier=rhythm-server

[Install]
WantedBy=default.target
EOF

    # Enable and start
    systemctl --user daemon-reload
    systemctl --user enable "$SERVICE_NAME"
    if [ -n "$NO_START" ]; then
        echo "Enabled but not started (--no-start)."
    else
        systemctl --user start "$SERVICE_NAME"
    fi

    echo ""
    echo "Installed successfully!"
    echo "  Server:   $bin_dir/rhythm-server"
    echo "  CLI:      $bin_dir/rhythm-cli"
    echo "  Data:     $data_dir"
    echo "  Service:  $service_path"
    echo ""
    echo "Commands:"
    echo "  rhythm-cli status"
    echo "  rhythm-cli stop"
    echo "  rhythm-cli start"
    echo "  rhythm-cli update"
    echo "  rhythm-cli uninstall"
    echo "  journalctl --user -u $SERVICE_NAME -f"
    echo ""
    echo "Note: For headless operation (no login session), run:"
    echo "  loginctl enable-linger $(whoami)"
}

linux_user_uninstall() {
    local bin_dir="$HOME/.local/bin"
    local bin_path="$bin_dir/rhythm-server"
    local cli_path="$bin_dir/rhythm-cli"
    local chipd_path="$bin_dir/rhythm-chipd"
    local data_dir="$HOME/.rhythm"
    local service_path="$HOME/.config/systemd/user/$SERVICE_NAME.service"

    # Stop and disable
    if systemctl --user is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
        echo "Stopping service..."
        systemctl --user stop "$SERVICE_NAME"
    fi
    if systemctl --user is-enabled --quiet "$SERVICE_NAME" 2>/dev/null; then
        systemctl --user disable "$SERVICE_NAME"
    fi

    # Remove service file
    if [ -f "$service_path" ]; then
        rm "$service_path"
        systemctl --user daemon-reload
        echo "Removed $service_path"
    fi

    # Remove binaries
    for bin in "$bin_path" "$cli_path" "$chipd_path"; do
        if [ -f "$bin" ]; then
            rm "$bin"
            echo "Removed $bin"
        fi
    done

    confirm_remove_data "$data_dir"

    echo ""
    echo "rhythm-server uninstalled."
}

# --- Main ---

case "$OS" in
    Darwin)
        if [ "$UNINSTALL" = true ]; then
            macos_uninstall
        else
            macos_install
        fi
        ;;
    Linux)
        if [ "$UNINSTALL" = true ]; then
            if [ "$USER_MODE" = true ]; then
                linux_user_uninstall
            else
                linux_system_uninstall
            fi
        else
            if [ "$USER_MODE" = true ]; then
                linux_user_install
            else
                linux_system_install
            fi
        fi
        ;;
    *)
        echo "Error: Unsupported OS: $OS"
        exit 1
        ;;
esac
