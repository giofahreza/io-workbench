#!/bin/sh
# Install io-workbench from a GitHub Release into the current user's home
# directory. Optional first-run setup stays local and authenticated by default.

set -eu

REPOSITORY=${IO_WORKBENCH_REPOSITORY:-giofahreza/io-workbench}
VERSION=${IO_WORKBENCH_VERSION:-latest}
PORT=${IO_WORKBENCH_PORT:-8787}
HOST=${IO_WORKBENCH_HOST:-127.0.0.1}
CONFIG_DIR=${IO_WORKBENCH_CONFIG_DIR:-}
WORKSPACE_ROOT=${IO_WORKBENCH_WORKSPACE_ROOT:-}
BIN_DIR=${IO_WORKBENCH_BIN_DIR:-}
NPM_PREFIX=${IO_WORKBENCH_NPM_PREFIX:-}
AUTOSTART_MODE=${IO_WORKBENCH_AUTOSTART:-auto}
LINGER_MODE=${IO_WORKBENCH_LINGER:-auto}
GATEWAY_MODE=${IO_WORKBENCH_INSTALL_IO_GATEWAY:-auto}
CODEX_MODE=${IO_WORKBENCH_INSTALL_CODEX:-auto}
CLAUDE_MODE=${IO_WORKBENCH_INSTALL_CLAUDE:-auto}
GEMINI_MODE=${IO_WORKBENCH_INSTALL_GEMINI:-auto}
CONFIGURE_CLIS_MODE=${IO_WORKBENCH_CONFIGURE_CLIS:-auto}
CHECK_PROVIDERS_MODE=${IO_WORKBENCH_CHECK_PROVIDERS:-auto}
TEST_PROVIDERS_MODE=${IO_WORKBENCH_TEST_PROVIDERS:-auto}
INTERACTIVE_MODE=${IO_WORKBENCH_INTERACTIVE:-auto}
TMP_DIR=''

PORT_EXPLICIT=0
HOST_EXPLICIT=0
CONFIG_DIR_EXPLICIT=0
WORKSPACE_ROOT_EXPLICIT=0
INTERACTIVE=0
AUTOSTART=0
LINGER=0
INSTALL_GATEWAY=0
INSTALL_CODEX=0
INSTALL_CLAUDE=0
INSTALL_GEMINI=0
CONFIGURE_CLIS=0
CHECK_PROVIDERS=0
TEST_PROVIDERS=0
SERVICE_WRITTEN=0
SERVICE_STARTED=0
HAD_MANAGED_SERVICE=0
DISABLE_AUTOSTART=0

LAUNCH_AGENT_LABEL='com.giofahreza.io-workbench'
MANAGED_MARKER='Managed by io-workbench install.sh'

[ -n "${IO_WORKBENCH_PORT:-}" ] && PORT_EXPLICIT=1
[ -n "${IO_WORKBENCH_HOST:-}" ] && HOST_EXPLICIT=1
[ -n "${IO_WORKBENCH_CONFIG_DIR:-}" ] && CONFIG_DIR_EXPLICIT=1
[ -n "${IO_WORKBENCH_WORKSPACE_ROOT:-}" ] && WORKSPACE_ROOT_EXPLICIT=1

usage() {
    printf '%s\n' \
        'Usage: install.sh [options]' \
        '' \
        'Install the matching io-workbench GitHub Release for Linux or macOS.' \
        'An interactive terminal receives a safe first-run setup questionnaire.' \
        '' \
        'Release options:' \
        '  --version <tag>          Install v0.1.0 (or 0.1.0), not latest.' \
        '' \
        'Runtime options:' \
        '  --host <IP>              Bind IPv4 address, ::1, or :: (default: 127.0.0.1).' \
        '  --port <port>            TCP port (default: 8787).' \
        '  --config-dir <path>      Data directory (default: ~/.io-workbench).' \
        '  --workspace-root <path>  Existing directory the UI may browse.' \
        '  --autostart              Enable/start a user service (Linux systemd or macOS LaunchAgent).' \
        '  --no-autostart           Do not create/start a service.' \
        '  --no-start               Alias for --no-autostart.' \
        '  --disable-autostart      Stop/remove a previously installer-managed macOS LaunchAgent, then exit.' \
        '  --linger                 Keep the Linux user manager after logout.' \
        '  --no-linger              Do not enable systemd user lingering.' \
        '' \
        'Optional host tools:' \
        '  --with-io-gateway | --without-io-gateway' \
        '  --with-codex | --without-codex' \
        '  --with-claude | --without-claude' \
        '  --with-gemini | --without-gemini' \
        '  --configure-clis | --no-configure-clis' \
        '  --check-providers | --no-check-providers' \
        '                         Check local version/auth status (no model request).' \
        '  --test-providers | --no-test-providers' \
        '                         Send one minimal read-only request per available CLI;' \
        '                         may use provider quota or incur cost.' \
        '' \
        'Interaction:' \
        '  --interactive            Require questions on /dev/tty.' \
        '  --non-interactive        Use explicit options and safe defaults.' \
        '  --help                   Show this help.' \
        '' \
        'Environment overrides:' \
        '  IO_WORKBENCH_VERSION, IO_WORKBENCH_REPOSITORY, IO_WORKBENCH_BIN_DIR' \
        '  IO_WORKBENCH_HOST, IO_WORKBENCH_PORT, IO_WORKBENCH_CONFIG_DIR' \
        '  IO_WORKBENCH_WORKSPACE_ROOT, IO_WORKBENCH_NPM_PREFIX' \
        '  IO_WORKBENCH_AUTOSTART, IO_WORKBENCH_LINGER' \
        '  IO_WORKBENCH_INSTALL_IO_GATEWAY, IO_WORKBENCH_INSTALL_CODEX' \
        '  IO_WORKBENCH_INSTALL_CLAUDE, IO_WORKBENCH_INSTALL_GEMINI' \
        '  IO_WORKBENCH_CONFIGURE_CLIS, IO_WORKBENCH_CHECK_PROVIDERS' \
        '  IO_WORKBENCH_TEST_PROVIDERS, IO_WORKBENCH_INTERACTIVE' \
        '' \
        'Choice values are auto, yes, or no. Non-interactive auto makes no' \
        'service, gateway, provider-CLI, provider-login, provider-check, or' \
        'live-model-request changes.'
}

note() {
    printf '%s\n' "io-workbench installer: $*"
}

warn() {
    printf '%s\n' "io-workbench installer: warning: $*" >&2
}

die() {
    printf '%s\n' "io-workbench installer: error: $*" >&2
    exit 1
}

cleanup() {
    if [ -n "${TMP_DIR:-}" ] && [ -d "$TMP_DIR" ]; then
        rm -rf "$TMP_DIR"
    fi
}

trap 'cleanup' 0
trap 'cleanup; exit 1' 1 2 3 15

is_valid_port() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
    esac
    [ "$1" -ge 1 ] 2>/dev/null && [ "$1" -le 65535 ] 2>/dev/null
}

validate_choice() {
    case "$2" in
        auto|yes|no) ;;
        *) die "$1 must be auto, yes, or no." ;;
    esac
}

tty_is_available() {
    [ -t 1 ] && ( : </dev/tty ) 2>/dev/null
}

ask_yes_no() {
    question=$1
    default=$2
    answer=''
    case "$default" in yes) suffix='[Y/n]' ;; no) suffix='[y/N]' ;; *) die 'invalid prompt default' ;; esac
    while :; do
        printf '%s ' "$question $suffix" >/dev/tty
        if ! IFS= read -r answer </dev/tty; then answer=''; fi
        case "$answer" in
            '') printf '%s' "$default"; return ;;
            y|Y|yes|YES|Yes) printf '%s' yes; return ;;
            n|N|no|NO|No) printf '%s' no; return ;;
            *) printf '%s\n' 'Please answer yes or no.' >/dev/tty ;;
        esac
    done
}

expand_home_path() {
    value=$1
    case "$value" in
        '~') printf '%s' "$HOME" ;;
        '~/'*) printf '%s/%s' "$HOME" "${value#\~/}" ;;
        *) printf '%s' "$value" ;;
    esac
}

absolute_path() {
    value=$(expand_home_path "$1")
    case "$value" in /*) printf '%s' "$value" ;; *) printf '%s/%s' "$PWD" "$value" ;; esac
}

is_ipv4_address() (
    address=$1
    case "$address" in ''|*[!0-9.]*) exit 1 ;; esac
    old_ifs=$IFS
    IFS=.
    set -- $address
    IFS=$old_ifs
    [ "$#" -eq 4 ] || exit 1
    for octet in "$@"; do
        case "$octet" in ''|*[!0-9]*) exit 1 ;; esac
        [ "$octet" -le 255 ] 2>/dev/null || exit 1
    done
)

is_ip_literal() {
    case "$1" in
        ::1|::) return 0 ;;
        *) is_ipv4_address "$1" ;;
    esac
}

is_loopback_host() {
    case "$1" in 127.0.0.1|::1) return 0 ;; *) return 1 ;; esac
}

port_is_in_use() {
    candidate=$1
    if command -v lsof >/dev/null 2>&1; then
        lsof -nP -iTCP:"$candidate" -sTCP:LISTEN >/dev/null 2>&1
        return
    fi
    if command -v ss >/dev/null 2>&1; then
        ss -ltnH 2>/dev/null | awk -v port="$candidate" '
            { value = $4; sub(/^.*:/, "", value); if (value == port) found = 1 }
            END { exit !found }
        '
        return
    fi
    return 1
}

ask_port() {
    default=$1
    value=''
    while :; do
        printf 'Workbench port [%s] ' "$default" >/dev/tty
        if ! IFS= read -r value </dev/tty; then value=''; fi
        [ -n "$value" ] || value=$default
        if ! is_valid_port "$value"; then
            printf '%s\n' 'Choose a whole-number TCP port from 1 through 65535.' >/dev/tty
            continue
        fi
        if port_is_in_use "$value" \
            && [ "$(ask_yes_no "Port $value appears to be in use. Keep it for an existing workbench upgrade?" no)" != yes ]; then
            continue
        fi
        printf '%s' "$value"
        return
    done
}

ask_host() {
    default=$1
    value=''
    while :; do
        printf 'Bind address [%s] ' "$default" >/dev/tty
        if ! IFS= read -r value </dev/tty; then value=''; fi
        [ -n "$value" ] || value=$default
        if ! is_ip_literal "$value"; then
            printf '%s\n' 'Use an IPv4 address, ::1, or ::; for example 127.0.0.1 or 0.0.0.0.' >/dev/tty
            continue
        fi
        if ! is_loopback_host "$value"; then
            printf '%s\n' 'A non-loopback listener lets other machines contact this host.' >/dev/tty
            printf '%s\n' 'Use HTTPS/WSS behind a trusted VPN, authenticated tunnel, or reverse proxy before remote use.' >/dev/tty
            [ "$(ask_yes_no 'Continue with this network-exposed bind address?' no)" = yes ] || continue
        fi
        printf '%s' "$value"
        return
    done
}

ask_directory() {
    label=$1
    default=$2
    require_existing=$3
    value=''
    while :; do
        printf '%s [%s] ' "$label" "$default" >/dev/tty
        if ! IFS= read -r value </dev/tty; then value=''; fi
        [ -n "$value" ] || value=$default
        value=$(absolute_path "$value")
        if [ "$require_existing" = yes ] && [ ! -d "$value" ]; then
            printf '%s\n' 'Choose an existing directory. It defines files the Web UI may browse.' >/dev/tty
            continue
        fi
        if [ -e "$value" ] && [ ! -d "$value" ]; then
            printf '%s\n' 'That path exists but is not a directory.' >/dev/tty
            continue
        fi
        printf '%s' "$value"
        return
    done
}

download() {
    url=$1
    destination=$2
    if command -v curl >/dev/null 2>&1; then
        curl --fail --location --silent --show-error --retry 3 --connect-timeout 15 --output "$destination" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget --quiet --tries=3 --timeout=30 --output-document="$destination" "$url"
    else
        die 'curl or wget is required to download a release.'
    fi
}

sha256_file() {
    file=$1
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file" | awk '{print $1}'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$file" | awk '{print $NF}'
    else
        die 'sha256sum, shasum, or openssl is required to verify the release.'
    fi
}

install_binary() {
    source=$1
    destination=$2
    directory=$(dirname "$destination")
    name=$(basename "$destination")
    staged="$directory/.$name.install.$$"
    rm -f "$staged"
    cp "$source" "$staged"
    chmod 755 "$staged"
    mv -f "$staged" "$destination"
}

systemd_escape() {
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

systemd_path_escape() {
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/ /\\x20/g' -e 's/%/%%/g'
}

env_line() {
    key=$1
    escaped=$(systemd_escape "$2")
    printf '%s="%s"\n' "$key" "$escaped"
}

shell_quote() {
    value=$(printf '%s' "$1" | sed "s/'/'\\\\''/g")
    printf "'%s'" "$value"
}

user_config_home() {
    case "${XDG_CONFIG_HOME:-}" in /*) printf '%s' "$XDG_CONFIG_HOME" ;; *) printf '%s/.config' "$HOME_ABS" ;; esac
}

is_managed_file() {
    [ -f "$1" ] \
        && [ ! -L "$1" ] \
        && grep -F "$MANAGED_MARKER" "$1" >/dev/null 2>&1
}

has_managed_systemd_service() {
    case "${XDG_CONFIG_HOME:-}" in
        /*) service_config_home=$XDG_CONFIG_HOME ;;
        *) service_config_home="$HOME/.config" ;;
    esac
    service_file="$service_config_home/systemd/user/io-workbench.service"
    is_managed_file "$service_file"
}

launch_agent_file() {
    launch_agent_home=${HOME_ABS:-$HOME}
    printf '%s/Library/LaunchAgents/%s.plist' "$launch_agent_home" "$LAUNCH_AGENT_LABEL"
}

is_current_user_owned() {
    [ "$(stat -f '%u' "$1" 2>/dev/null || true)" = "$(id -u)" ]
}

has_managed_launch_agent() {
    agent_file=$(launch_agent_file)
    is_managed_file "$agent_file" && is_current_user_owned "$agent_file"
}

xml_escape() {
    printf '%s' "$1" | sed \
        -e 's/&/\&amp;/g' \
        -e 's/</\&lt;/g' \
        -e 's/>/\&gt;/g' \
        -e 's/"/\&quot;/g'
}

plist_indented_string() {
    printf '%s<string>%s</string>\n' "$1" "$(xml_escape "$2")"
}

health_url() {
    health_host=$HOST
    case "$health_host" in 0.0.0.0) health_host=127.0.0.1 ;; ::) health_host=::1 ;; esac
    case "$health_host" in *:*) printf 'http://[%s]:%s/health' "$health_host" "$PORT" ;; *) printf 'http://%s:%s/health' "$health_host" "$PORT" ;; esac
}

wait_for_health() {
    attempt=0
    url=$(health_url)
    while [ "$attempt" -lt 15 ]; do
        if command -v curl >/dev/null 2>&1 \
            && curl --noproxy '*' --fail --silent --show-error --max-time 2 "$url" >/dev/null 2>&1; then
            return 0
        fi
        attempt=$((attempt + 1))
        [ "$attempt" -lt 15 ] && sleep 1
    done
    return 1
}

write_systemd_service() {
    command -v systemctl >/dev/null 2>&1 || return 1
    config_home=$(user_config_home)
    service_dir="$config_home/systemd/user"
    service_file="$service_dir/io-workbench.service"
    env_dir="$config_home/io-workbench"
    env_file="$env_dir/io-workbench.env"

    if [ -e "$service_file" ] && ! is_managed_file "$service_file"; then
        warn "not overwriting unrecognized user unit: $service_file"
        warn 'Edit that unit yourself, or remove it before asking this installer to manage autostart.'
        return 2
    fi

    mkdir -p "$service_dir" "$env_dir"
    env_tmp="$env_dir/.io-workbench.env.install.$$"
    unit_tmp="$service_dir/.io-workbench.service.install.$$"
    env_file_escaped=$(systemd_path_escape "$env_file")
    workspace_escaped=$(systemd_path_escape "$WORKSPACE_ROOT_ABS")
    binary_escaped=$(systemd_escape "$WORKBENCH_BINARY")

    umask 077
    {
        env_line HOME "$HOME_ABS"
        env_line PATH "$SERVICE_PATH"
        env_line IO_WORKBENCH_HOST "$HOST"
        env_line IO_WORKBENCH_PORT "$PORT"
        env_line IO_WORKBENCH_CONFIG_DIR "$CONFIG_DIR_ABS"
        env_line IO_WORKBENCH_WORKSPACE_ROOT "$WORKSPACE_ROOT_ABS"
        env_line IO_WORKBENCH_AUTH_REQUIRED true
    } > "$env_tmp"
    chmod 600 "$env_tmp"
    mv -f "$env_tmp" "$env_file"
    {
        printf '%s\n' "# $MANAGED_MARKER"
        printf '%s\n' '[Unit]' 'Description=io-workbench user host' 'After=network-online.target' 'Wants=network-online.target' ''
        printf '%s\n' '[Service]' 'Type=simple'
        printf 'EnvironmentFile=%s\n' "$env_file_escaped"
        printf 'WorkingDirectory=%s\n' "$workspace_escaped"
        printf 'ExecStart="%s" start\n' "$binary_escaped"
        printf '%s\n' 'Restart=on-failure' 'RestartSec=5' '' '[Install]' 'WantedBy=default.target'
    } > "$unit_tmp"
    chmod 644 "$unit_tmp"
    mv -f "$unit_tmp" "$service_file"
    SERVICE_WRITTEN=1

    systemctl --user daemon-reload >/dev/null 2>&1 \
        && systemctl --user enable io-workbench.service >/dev/null 2>&1 \
        || return 1
    if systemctl --user is-active --quiet io-workbench.service; then
        systemctl --user restart io-workbench.service >/dev/null 2>&1
    else
        systemctl --user start io-workbench.service >/dev/null 2>&1
    fi
}

write_launch_agent() {
    command -v launchctl >/dev/null 2>&1 || return 1
    agent_file=$(launch_agent_file)
    agent_dir=$(dirname "$agent_file")
    log_file="$CONFIG_DIR_ABS/io-workbench.launchd.log"

    if { [ -e "$agent_file" ] || [ -L "$agent_file" ]; } \
        && { [ -L "$agent_file" ] || ! is_current_user_owned "$agent_file" || ! is_managed_file "$agent_file"; }; then
        warn "not overwriting an unrecognized or non-user-owned LaunchAgent: $agent_file"
        warn 'Remove or repair that file yourself before asking this installer to manage autostart.'
        return 2
    fi

    mkdir -p "$agent_dir"
    agent_tmp="$agent_dir/.${LAUNCH_AGENT_LABEL}.plist.install.$$"
    umask 077
    {
        printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>'
        printf '%s\n' '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
        printf '%s\n' '<plist version="1.0">'
        printf '%s\n' "<!-- $MANAGED_MARKER -->"
        printf '%s\n' '<dict>'
        printf '%s\n' '  <key>Label</key>'
        plist_indented_string '  ' "$LAUNCH_AGENT_LABEL"
        printf '%s\n' '  <key>ProgramArguments</key>' '  <array>'
        plist_indented_string '    ' "$WORKBENCH_BINARY"
        plist_indented_string '    ' start
        printf '%s\n' '  </array>'
        printf '%s\n' '  <key>WorkingDirectory</key>'
        plist_indented_string '  ' "$WORKSPACE_ROOT_ABS"
        printf '%s\n' '  <key>EnvironmentVariables</key>' '  <dict>'
        for env_pair in \
            "HOME=$HOME_ABS" \
            "PATH=$SERVICE_PATH" \
            "IO_WORKBENCH_HOST=$HOST" \
            "IO_WORKBENCH_PORT=$PORT" \
            "IO_WORKBENCH_CONFIG_DIR=$CONFIG_DIR_ABS" \
            "IO_WORKBENCH_WORKSPACE_ROOT=$WORKSPACE_ROOT_ABS" \
            'IO_WORKBENCH_AUTH_REQUIRED=true'; do
            env_key=${env_pair%%=*}
            env_value=${env_pair#*=}
            printf '    <key>%s</key>\n' "$env_key"
            plist_indented_string '    ' "$env_value"
        done
        printf '%s\n' '  </dict>'
        printf '%s\n' '  <key>RunAtLoad</key>' '  <true/>'
        printf '%s\n' '  <key>KeepAlive</key>' '  <dict>' '    <key>SuccessfulExit</key>' '    <false/>' '  </dict>'
        printf '%s\n' '  <key>ProcessType</key>' '  <string>Background</string>'
        printf '%s\n' '  <key>StandardOutPath</key>'
        plist_indented_string '  ' "$log_file"
        printf '%s\n' '  <key>StandardErrorPath</key>'
        plist_indented_string '  ' "$log_file"
        printf '%s\n' '</dict>' '</plist>'
    } > "$agent_tmp"
    chmod 600 "$agent_tmp"
    if command -v plutil >/dev/null 2>&1 && ! plutil -lint "$agent_tmp" >/dev/null 2>&1; then
        rm -f "$agent_tmp"
        warn "generated LaunchAgent did not pass plutil validation: $agent_file"
        return 1
    fi

    user_id=$(id -u)
    agent_domain="gui/$user_id"
    agent_service="$agent_domain/$LAUNCH_AGENT_LABEL"
    # The file path is already verified as installer-managed; do not touch a
    # different LaunchAgent merely because it happens to use this label.
    launchctl bootout "$agent_domain" "$agent_file" >/dev/null 2>&1 || true
    mv -f "$agent_tmp" "$agent_file"
    SERVICE_WRITTEN=1

    launchctl bootstrap "$agent_domain" "$agent_file" >/dev/null 2>&1 \
        && launchctl kickstart -k "$agent_service" >/dev/null 2>&1
}

disable_managed_launch_agent() {
    agent_file=$(launch_agent_file)
    if [ ! -e "$agent_file" ] && [ ! -L "$agent_file" ]; then
        note 'No installer-managed macOS LaunchAgent is present.'
        return 0
    fi
    if [ -L "$agent_file" ] || ! is_current_user_owned "$agent_file" || ! is_managed_file "$agent_file"; then
        warn "refusing to remove an unrecognized or non-user-owned LaunchAgent: $agent_file"
        return 1
    fi

    if command -v launchctl >/dev/null 2>&1; then
        user_id=$(id -u)
        agent_domain="gui/$user_id"
        launchctl bootout "$agent_domain" "$agent_file" >/dev/null 2>&1 || true
    fi
    rm -f "$agent_file"
    note "Removed installer-managed macOS LaunchAgent: $agent_file"
}

install_gateway() {
    installer="$TMP_DIR/io-gateway-install.sh"
    note 'Downloading the optional IO Gateway installer.'
    download 'https://github.com/giofahreza/io-gateway/releases/latest/download/install.sh' "$installer"
    chmod 700 "$installer"
    IO_GATEWAY_INTERACTIVE=no IO_GATEWAY_AUTOSTART=no sh "$installer" --no-start
}

install_provider_cli() {
    label=$1
    package=$2
    command_name=$3
    if ! command -v npm >/dev/null 2>&1; then
        warn "cannot install $label: npm and Node.js are not available."
        warn "Install Node.js with your approved platform method, then rerun with --with-$command_name."
        return 1
    fi
    note "Installing/updating $label in $NPM_PREFIX_ABS."
    npm install --global --prefix "$NPM_PREFIX_ABS" "$package"
}

provider_binary() {
    command_name=$1
    if [ -x "$NPM_BIN_DIR/$command_name" ]; then
        printf '%s' "$NPM_BIN_DIR/$command_name"
    elif command -v "$command_name" >/dev/null 2>&1; then
        command -v "$command_name"
    else
        return 1
    fi
}

configure_provider_cli() {
    command_name=$1
    label=$2
    provider=$(provider_binary "$command_name" || true)
    if [ -z "$provider" ]; then
        warn "$label is unavailable, so its sign-in flow was skipped."
        return
    fi
    note "Opening the native $label sign-in/setup flow. Complete it, then return here."
    case "$command_name" in
        codex) "$provider" login ;;
        claude) "$provider" auth login ;;
        gemini) "$provider" ;;
    esac
}

provider_readiness_check() {
    command_name=$1
    label=$2
    provider=$(provider_binary "$command_name" || true)
    if [ -z "$provider" ]; then
        note "$label: not found; install it or add it to PATH before using that provider."
        return 0
    fi

    version_output="$TMP_DIR/provider-$command_name-version.out"
    if "$provider" --version >"$version_output" 2>&1; then
        version_line=$(tr -d '\r' < "$version_output" | sed -n '/[^[:space:]]/{p;q;}')
        if [ -n "$version_line" ]; then
            note "$label: version check passed ($version_line)."
        else
            note "$label: version check passed."
        fi
    else
        warn "$label: its version command failed; update or repair the CLI before using it in io-workbench."
    fi

    auth_output="$TMP_DIR/provider-$command_name-auth.out"
    case "$command_name" in
        codex)
            if "$provider" login status >"$auth_output" 2>&1; then
                note "$label: native login status reports authenticated."
            else
                warn "$label: native login status did not confirm authentication. Run: codex login"
            fi
            ;;
        claude)
            if "$provider" auth status --json >"$auth_output" 2>&1 \
                && grep -Eq '"loggedIn"[[:space:]]*:[[:space:]]*true' "$auth_output"; then
                note "$label: native auth status reports authenticated."
            else
                warn "$label: native auth status did not confirm authentication. Run: claude auth login"
            fi
            ;;
        gemini)
            if [ -n "${GEMINI_API_KEY:-}" ] || [ -n "${GOOGLE_API_KEY:-}" ] \
                || [ -n "${GOOGLE_APPLICATION_CREDENTIALS:-}" ] \
                || [ -f "$HOME_ABS/.gemini/oauth_creds.json" ]; then
                note "$label: possible credential configuration detected (not read)."
            else
                note "$label: no portable offline auth-status command is available; native setup or a live test confirms access."
            fi
            ;;
    esac
}

provider_readiness_checks() {
    note 'Checking available provider CLIs locally (version and native auth status only; no model request).'
    provider_readiness_check codex 'Codex CLI'
    provider_readiness_check claude 'Claude Code'
    provider_readiness_check gemini 'Gemini CLI'
}

timeout_runner_available() {
    command -v timeout >/dev/null 2>&1 \
        || command -v gtimeout >/dev/null 2>&1 \
        || command -v perl >/dev/null 2>&1
}

run_with_timeout() {
    timeout_seconds=$1
    shift
    if command -v timeout >/dev/null 2>&1; then
        timeout "$timeout_seconds" "$@"
    elif command -v gtimeout >/dev/null 2>&1; then
        gtimeout "$timeout_seconds" "$@"
    else
        # macOS usually has Perl even when GNU coreutils (and timeout) are not
        # installed. alarm survives exec and bounds the provider subprocess.
        perl -e 'my $seconds = shift @ARGV; alarm $seconds; exec @ARGV or die "could not start provider: $!\n";' "$timeout_seconds" "$@"
    fi
}

provider_live_test() {
    command_name=$1
    label=$2
    provider=$(provider_binary "$command_name" || true)
    if [ -z "$provider" ]; then
        note "$label live test: skipped because the CLI is not installed."
        return 0
    fi
    if ! timeout_runner_available; then
        warn "$label live test: skipped because timeout, gtimeout, and perl are unavailable to enforce the 90-second limit."
        return 0
    fi

    probe_dir="$TMP_DIR/provider-readiness-workspace"
    if ! mkdir -p "$probe_dir"; then
        warn "$label live test: skipped because a temporary probe workspace could not be created."
        return 0
    fi
    chmod 700 "$probe_dir" 2>/dev/null || true
    output="$TMP_DIR/provider-$command_name-live.out"
    prompt='Reply with exactly READY and nothing else. Do not use tools, read files, execute commands, or change anything.'
    note "$label live test: sending one minimal provider request (read-only; may use quota or incur cost)."

    case "$command_name" in
        codex)
            if (
                cd "$probe_dir"
                run_with_timeout 90 "$provider" exec --ephemeral --sandbox read-only \
                    --skip-git-repo-check --ignore-rules --color never "$prompt"
            ) >"$output" 2>&1; then test_status=0; else test_status=$?; fi
            ;;
        claude)
            if (
                cd "$probe_dir"
                run_with_timeout 90 "$provider" --print --no-session-persistence \
                    --permission-mode plan --tools '' --no-chrome --strict-mcp-config "$prompt"
            ) >"$output" 2>&1; then test_status=0; else test_status=$?; fi
            ;;
        gemini)
            if (
                cd "$probe_dir"
                run_with_timeout 90 "$provider" --prompt "$prompt" --approval-mode plan --sandbox \
                    --extensions none --allowed-mcp-server-names none --output-format json --skip-trust
            ) >"$output" 2>&1; then test_status=0; else test_status=$?; fi
            ;;
        *)
            warn "$label live test: unsupported provider command."
            return 0
            ;;
    esac

    if [ "$test_status" -eq 0 ] && grep -Eiq '(^|[^[:alnum:]_])READY([^[:alnum:]_]|$)' "$output"; then
        note "$label live test: passed."
        return 0
    fi
    if grep -Eiq 'unknown (option|argument|flag)|unrecognized (option|argument|flag)|invalid (option|argument|flag)' "$output"; then
        warn "$label live test: the installed CLI rejected a safe-test flag. Update the CLI, then retry --test-providers."
    elif [ "$test_status" -eq 124 ] || [ "$test_status" -eq 142 ] || [ "$test_status" -eq 137 ]; then
        warn "$label live test: timed out after 90 seconds. Check provider connectivity and authentication, then retry --test-providers."
    elif [ "$test_status" -eq 0 ]; then
        warn "$label live test: the request finished but did not return the expected READY response. Retry it before relying on the provider."
    else
        warn "$label live test: failed with exit code $test_status. Check its native login/status command, then retry --test-providers."
    fi
    return 0
}

provider_live_tests() {
    note 'Running explicitly requested live provider readiness tests in a temporary empty workspace.'
    provider_live_test codex 'Codex CLI'
    provider_live_test claude 'Claude Code'
    provider_live_test gemini 'Gemini CLI'
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) [ "$#" -ge 2 ] || die '--version needs a release tag.'; VERSION=$2; shift 2 ;;
        --version=*) VERSION=$(printf '%s' "$1" | sed 's/^--version=//'); shift ;;
        --port) [ "$#" -ge 2 ] || die '--port needs a TCP port number.'; PORT=$2; PORT_EXPLICIT=1; shift 2 ;;
        --port=*) PORT=$(printf '%s' "$1" | sed 's/^--port=//'); PORT_EXPLICIT=1; shift ;;
        --host) [ "$#" -ge 2 ] || die '--host needs an IP address.'; HOST=$2; HOST_EXPLICIT=1; shift 2 ;;
        --host=*) HOST=$(printf '%s' "$1" | sed 's/^--host=//'); HOST_EXPLICIT=1; shift ;;
        --config-dir) [ "$#" -ge 2 ] || die '--config-dir needs a path.'; CONFIG_DIR=$2; CONFIG_DIR_EXPLICIT=1; shift 2 ;;
        --config-dir=*) CONFIG_DIR=$(printf '%s' "$1" | sed 's/^--config-dir=//'); CONFIG_DIR_EXPLICIT=1; shift ;;
        --workspace-root) [ "$#" -ge 2 ] || die '--workspace-root needs a path.'; WORKSPACE_ROOT=$2; WORKSPACE_ROOT_EXPLICIT=1; shift 2 ;;
        --workspace-root=*) WORKSPACE_ROOT=$(printf '%s' "$1" | sed 's/^--workspace-root=//'); WORKSPACE_ROOT_EXPLICIT=1; shift ;;
        --autostart|--start) AUTOSTART_MODE=yes; DISABLE_AUTOSTART=0; shift ;;
        --no-autostart|--no-start) AUTOSTART_MODE=no; DISABLE_AUTOSTART=0; shift ;;
        --disable-autostart) AUTOSTART_MODE=no; DISABLE_AUTOSTART=1; shift ;;
        --linger) LINGER_MODE=yes; shift ;;
        --no-linger) LINGER_MODE=no; shift ;;
        --with-io-gateway) GATEWAY_MODE=yes; shift ;;
        --without-io-gateway|--no-io-gateway) GATEWAY_MODE=no; shift ;;
        --with-codex) CODEX_MODE=yes; shift ;;
        --without-codex|--no-codex) CODEX_MODE=no; shift ;;
        --with-claude) CLAUDE_MODE=yes; shift ;;
        --without-claude|--no-claude) CLAUDE_MODE=no; shift ;;
        --with-gemini) GEMINI_MODE=yes; shift ;;
        --without-gemini|--no-gemini) GEMINI_MODE=no; shift ;;
        --configure-clis) CONFIGURE_CLIS_MODE=yes; shift ;;
        --no-configure-clis) CONFIGURE_CLIS_MODE=no; shift ;;
        --check-providers) CHECK_PROVIDERS_MODE=yes; shift ;;
        --no-check-providers) CHECK_PROVIDERS_MODE=no; shift ;;
        --test-providers) TEST_PROVIDERS_MODE=yes; shift ;;
        --no-test-providers) TEST_PROVIDERS_MODE=no; shift ;;
        --interactive) INTERACTIVE_MODE=yes; shift ;;
        --non-interactive|--yes) INTERACTIVE_MODE=no; shift ;;
        --help|-h) usage; exit 0 ;;
        *) die "unknown option: $1 (run with --help for usage)" ;;
    esac
done

validate_choice IO_WORKBENCH_AUTOSTART "$AUTOSTART_MODE"
validate_choice IO_WORKBENCH_LINGER "$LINGER_MODE"
validate_choice IO_WORKBENCH_INSTALL_IO_GATEWAY "$GATEWAY_MODE"
validate_choice IO_WORKBENCH_INSTALL_CODEX "$CODEX_MODE"
validate_choice IO_WORKBENCH_INSTALL_CLAUDE "$CLAUDE_MODE"
validate_choice IO_WORKBENCH_INSTALL_GEMINI "$GEMINI_MODE"
validate_choice IO_WORKBENCH_CONFIGURE_CLIS "$CONFIGURE_CLIS_MODE"
validate_choice IO_WORKBENCH_CHECK_PROVIDERS "$CHECK_PROVIDERS_MODE"
validate_choice IO_WORKBENCH_TEST_PROVIDERS "$TEST_PROVIDERS_MODE"
validate_choice IO_WORKBENCH_INTERACTIVE "$INTERACTIVE_MODE"

[ -n "${HOME:-}" ] || die 'HOME is not set; choose a user account before running the installer.'
[ -n "$CONFIG_DIR" ] || CONFIG_DIR="$HOME/.io-workbench"
[ -n "$WORKSPACE_ROOT" ] || WORKSPACE_ROOT="$HOME"
[ -n "$BIN_DIR" ] || BIN_DIR="$HOME/.local/bin"
[ -n "$NPM_PREFIX" ] || NPM_PREFIX="$HOME/.local"
is_valid_port "$PORT" || die "invalid port: $PORT. Choose a whole-number TCP port from 1 through 65535."
is_ip_literal "$HOST" || die "invalid host: $HOST. Use an IPv4 address, ::1, or ::."

case "$REPOSITORY" in */*) ;; *) die 'IO_WORKBENCH_REPOSITORY must be in owner/repository form.' ;; esac
case "$REPOSITORY" in *[!A-Za-z0-9._/-]*|*//*|/*|*/) die 'IO_WORKBENCH_REPOSITORY contains unsupported characters.' ;; esac

case "$INTERACTIVE_MODE" in
    yes) tty_is_available || die '--interactive requires a controlling terminal; use --non-interactive for automation.'; INTERACTIVE=1 ;;
    auto) tty_is_available && INTERACTIVE=1 ;;
esac

OS_NAME=$(uname -s 2>/dev/null || true)
MACHINE=$(uname -m 2>/dev/null || true)
case "$OS_NAME" in
    Linux)
        PLATFORM=linux
        case "$MACHINE" in x86_64|amd64) TARGET=linux-x86_64 ;; aarch64|arm64) TARGET=linux-aarch64 ;; *) die "unsupported Linux CPU architecture: ${MACHINE:-unknown}." ;; esac
        ;;
    Darwin)
        PLATFORM=macos
        if [ "$MACHINE" = x86_64 ] && command -v sysctl >/dev/null 2>&1 \
            && [ "$(sysctl -in sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then MACHINE=arm64; fi
        case "$MACHINE" in x86_64|amd64) TARGET=macos-x86_64 ;; aarch64|arm64) TARGET=macos-aarch64 ;; *) die "unsupported macOS CPU architecture: ${MACHINE:-unknown}." ;; esac
        ;;
    *) die "unsupported operating system: ${OS_NAME:-unknown}. Use install.ps1 on Windows." ;;
esac

if [ "$DISABLE_AUTOSTART" -eq 1 ]; then
    [ "$PLATFORM" = macos ] || die '--disable-autostart is available only on macOS. Use your systemd tooling to disable a Linux user service.'
    disable_managed_launch_agent || die 'the macOS LaunchAgent was not removed.'
    exit 0
fi

case "$PLATFORM" in
    linux) has_managed_systemd_service && HAD_MANAGED_SERVICE=1 ;;
    macos) has_managed_launch_agent && HAD_MANAGED_SERVICE=1 ;;
esac

if [ "$INTERACTIVE" -eq 1 ]; then
    printf '\n%s\n' 'io-workbench first-run setup' >/dev/tty
    printf '%s\n' 'The host runs code, terminals, databases, and provider CLIs for the directories you allow.' >/dev/tty
    printf '%s\n' 'Authentication remains enabled. No password, provider API key, token, or OTP secret is requested.' >/dev/tty
    if [ "$PORT_EXPLICIT" -eq 1 ]; then
        port_is_in_use "$PORT" && warn "port $PORT appears to be in use; this is normal when upgrading a running workbench."
    else
        PORT=$(ask_port "$PORT")
    fi
    if [ "$HOST_EXPLICIT" -eq 0 ]; then
        HOST=$(ask_host "$HOST")
    elif ! is_loopback_host "$HOST"; then
        printf '%s\n' 'The selected bind address accepts network connections; secure it with HTTPS/WSS and a trusted boundary.' >/dev/tty
        [ "$(ask_yes_no 'Continue with this network-exposed bind address?' no)" = yes ] || die 'installation cancelled before downloading a release.'
    fi
    [ "$CONFIG_DIR_EXPLICIT" -eq 1 ] || CONFIG_DIR=$(ask_directory 'Data/configuration directory' "$CONFIG_DIR" no)
    [ "$WORKSPACE_ROOT_EXPLICIT" -eq 1 ] || WORKSPACE_ROOT=$(ask_directory 'Workspace root (existing directory)' "$WORKSPACE_ROOT" yes)
    CONFIG_DIR=$(absolute_path "$CONFIG_DIR")
    WORKSPACE_ROOT=$(absolute_path "$WORKSPACE_ROOT")
    if [ "$WORKSPACE_ROOT" = / ]; then
        printf '%s\n' 'A workspace root of / grants the Web UI authority over the entire host filesystem.' >/dev/tty
        [ "$(ask_yes_no 'Keep / as the workspace root?' no)" = yes ] || die 'installation cancelled before downloading a release.'
    fi
    case "$AUTOSTART_MODE" in
        yes) AUTOSTART=1 ;;
        no) ;;
        auto)
            autostart_default=no
            [ "$HAD_MANAGED_SERVICE" -eq 0 ] || autostart_default=yes
            case "$PLATFORM" in
                linux) autostart_question='Enable and start a systemd user service at sign-in?' ;;
                macos) autostart_question='Enable and start a macOS LaunchAgent at sign-in?' ;;
            esac
            if [ "$(ask_yes_no "$autostart_question" "$autostart_default")" = yes ]; then AUTOSTART=1; fi
            ;;
    esac
    if [ "$AUTOSTART" -eq 1 ] && [ "$PLATFORM" = linux ]; then
        case "$LINGER_MODE" in
            yes) LINGER=1 ;;
            no) ;;
            auto)
                printf '%s\n' 'Linger keeps the user manager available after logout/reboot.' >/dev/tty
                [ "$(ask_yes_no 'Enable linger for this user?' no)" = yes ] && LINGER=1
                ;;
        esac
    fi
    case "$GATEWAY_MODE" in yes) INSTALL_GATEWAY=1 ;; no) ;; auto) [ "$(ask_yes_no 'Install the optional IO Gateway (localhost-only, separate setup)?' no)" = yes ] && INSTALL_GATEWAY=1 ;; esac
    case "$CODEX_MODE" in yes) INSTALL_CODEX=1 ;; no) ;; auto) [ "$(ask_yes_no 'Install/update the Codex CLI with npm?' no)" = yes ] && INSTALL_CODEX=1 ;; esac
    case "$CLAUDE_MODE" in yes) INSTALL_CLAUDE=1 ;; no) ;; auto) [ "$(ask_yes_no 'Install/update Claude Code with npm?' no)" = yes ] && INSTALL_CLAUDE=1 ;; esac
    case "$GEMINI_MODE" in yes) INSTALL_GEMINI=1 ;; no) ;; auto) [ "$(ask_yes_no 'Install/update Gemini CLI with npm?' no)" = yes ] && INSTALL_GEMINI=1 ;; esac
    case "$CONFIGURE_CLIS_MODE" in
        yes) CONFIGURE_CLIS=1 ;;
        no) ;;
        auto)
            if [ "$INSTALL_CODEX" -eq 1 ] || [ "$INSTALL_CLAUDE" -eq 1 ] || [ "$INSTALL_GEMINI" -eq 1 ] \
                || command -v codex >/dev/null 2>&1 || command -v claude >/dev/null 2>&1 || command -v gemini >/dev/null 2>&1; then
                printf '%s\n' 'Native login can open a browser or interactive session; credentials stay with the provider CLI.' >/dev/tty
                [ "$(ask_yes_no 'Open selected provider login/setup flows after installation?' no)" = yes ] && CONFIGURE_CLIS=1
            fi
            ;;
    esac
    case "$CHECK_PROVIDERS_MODE" in
        yes) CHECK_PROVIDERS=1 ;;
        no) ;;
        auto)
            if [ "$INSTALL_CODEX" -eq 1 ] || [ "$INSTALL_CLAUDE" -eq 1 ] || [ "$INSTALL_GEMINI" -eq 1 ] \
                || command -v codex >/dev/null 2>&1 || command -v claude >/dev/null 2>&1 || command -v gemini >/dev/null 2>&1; then
                printf '%s\n' 'Local readiness checks run version and native authentication-status commands only; they do not send a model request.' >/dev/tty
                [ "$(ask_yes_no 'Check available provider CLI readiness after installation?' yes)" = yes ] && CHECK_PROVIDERS=1
            fi
            ;;
    esac
    case "$TEST_PROVIDERS_MODE" in
        yes) TEST_PROVIDERS=1 ;;
        no) ;;
        auto)
            if [ "$INSTALL_CODEX" -eq 1 ] || [ "$INSTALL_CLAUDE" -eq 1 ] || [ "$INSTALL_GEMINI" -eq 1 ] \
                || command -v codex >/dev/null 2>&1 || command -v claude >/dev/null 2>&1 || command -v gemini >/dev/null 2>&1; then
                printf '%s\n' 'A live test sends one tiny request to every available provider. It uses a temporary empty workspace and safe read-only/no-tool modes, but may use provider quota or incur cost.' >/dev/tty
                [ "$(ask_yes_no 'Run live provider readiness tests after installation?' no)" = yes ] && TEST_PROVIDERS=1
            fi
            ;;
    esac
    printf '\n%s\n' 'Installation summary' >/dev/tty
    printf '%s\n' "  Bind: $HOST:$PORT" "  Config: $CONFIG_DIR" "  Workspace authority: $WORKSPACE_ROOT" >/dev/tty
    if [ "$AUTOSTART" -eq 1 ]; then
        [ "$PLATFORM" = linux ] && startup_summary='systemd --user service' || startup_summary='macOS LaunchAgent'
        printf '%s\n' "  Startup: $startup_summary" >/dev/tty
    elif [ "$DISABLE_AUTOSTART" -eq 1 ]; then
        printf '%s\n' '  Startup: remove this installer-managed macOS LaunchAgent' >/dev/tty
    else
        printf '%s\n' '  Startup: manual' >/dev/tty
    fi
    [ "$INSTALL_GATEWAY" -eq 1 ] && printf '%s\n' '  IO Gateway: install only (no auto-start)' >/dev/tty
    [ "$INSTALL_CODEX" -eq 1 ] && printf '%s\n' '  Codex CLI: npm install/update' >/dev/tty
    [ "$INSTALL_CLAUDE" -eq 1 ] && printf '%s\n' '  Claude Code: npm install/update' >/dev/tty
    [ "$INSTALL_GEMINI" -eq 1 ] && printf '%s\n' '  Gemini CLI: npm install/update' >/dev/tty
    [ "$CONFIGURE_CLIS" -eq 1 ] && printf '%s\n' '  Provider login: native flow(s) after installation' >/dev/tty
    [ "$CHECK_PROVIDERS" -eq 1 ] && printf '%s\n' '  Provider readiness: local version/auth checks (no model request)' >/dev/tty
    [ "$TEST_PROVIDERS" -eq 1 ] && printf '%s\n' '  Provider readiness: one live read-only request per available CLI (may use quota/cost)' >/dev/tty
    [ "$(ask_yes_no 'Download and apply this setup?' yes)" = yes ] || die 'installation cancelled before downloading a release.'
else
    [ "$AUTOSTART_MODE" = yes ] && AUTOSTART=1
    if [ "$AUTOSTART_MODE" = auto ] && [ "$HAD_MANAGED_SERVICE" -eq 1 ]; then AUTOSTART=1; fi
    [ "$LINGER_MODE" = yes ] && LINGER=1
    [ "$GATEWAY_MODE" = yes ] && INSTALL_GATEWAY=1
    [ "$CODEX_MODE" = yes ] && INSTALL_CODEX=1
    [ "$CLAUDE_MODE" = yes ] && INSTALL_CLAUDE=1
    [ "$GEMINI_MODE" = yes ] && INSTALL_GEMINI=1
    [ "$CONFIGURE_CLIS_MODE" = yes ] && CONFIGURE_CLIS=1
    [ "$CHECK_PROVIDERS_MODE" = yes ] && CHECK_PROVIDERS=1
    [ "$TEST_PROVIDERS_MODE" = yes ] && TEST_PROVIDERS=1
    [ "$CONFIGURE_CLIS" -eq 0 ] || die '--configure-clis needs a controlling terminal; use --interactive to complete native login.'
    note 'No interactive terminal detected; installing the binary only unless explicit options selected more.'
fi

if [ "$LINGER" -eq 1 ] && { [ "$AUTOSTART" -eq 0 ] || [ "$PLATFORM" != linux ]; }; then
    warn '--linger is ignored unless a Linux systemd user service is enabled.'
    LINGER=0
fi
if ! is_loopback_host "$HOST"; then
    warn "io-workbench will bind to $HOST. Authentication remains enabled, but remote use still needs an HTTPS/WSS network boundary."
fi

CONFIG_DIR=$(absolute_path "$CONFIG_DIR")
WORKSPACE_ROOT=$(absolute_path "$WORKSPACE_ROOT")
BIN_DIR=$(absolute_path "$BIN_DIR")
NPM_PREFIX=$(absolute_path "$NPM_PREFIX")
[ -d "$WORKSPACE_ROOT" ] || die "workspace root does not exist: $WORKSPACE_ROOT"
[ ! -e "$CONFIG_DIR" ] || [ -d "$CONFIG_DIR" ] || die "config path exists but is not a directory: $CONFIG_DIR"

TMP_BASE=$(printenv TMPDIR 2>/dev/null || printf '%s' /tmp)
TMP_DIR=$(mktemp -d "$TMP_BASE/io-workbench-install.XXXXXX") || die 'could not create a temporary directory.'

if [ "$VERSION" = latest ]; then
    RELEASE_JSON="$TMP_DIR/release.json"
    note "Resolving the latest release from $REPOSITORY."
    download "https://api.github.com/repos/$REPOSITORY/releases/latest" "$RELEASE_JSON"
    TAG=$(tr '\n' ' ' < "$RELEASE_JSON" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
    [ -n "$TAG" ] || die 'could not read tag_name from the GitHub release response.'
else
    case "$VERSION" in v[0-9]*) TAG=$VERSION ;; [0-9]*) TAG="v$VERSION" ;; *) die "invalid release version: $VERSION" ;; esac
fi
case "$TAG" in v[0-9]*) ;; *) die "invalid GitHub release tag: $TAG" ;; esac
case "$TAG" in *[!0-9A-Za-z._-]*) die "invalid GitHub release tag: $TAG" ;; esac

ASSET_NAME="io-workbench-$TAG-$TARGET.tar.gz"
RELEASE_BASE="https://github.com/$REPOSITORY/releases/download/$TAG"
ARCHIVE="$TMP_DIR/$ASSET_NAME"
SUMS_FILE="$TMP_DIR/SHA256SUMS"
note "Downloading $ASSET_NAME."
download "$RELEASE_BASE/$ASSET_NAME" "$ARCHIVE"
download "$RELEASE_BASE/SHA256SUMS" "$SUMS_FILE"
EXPECTED_SHA256=$(awk -v filename="$ASSET_NAME" '$2 == filename || $2 == ("*" filename) { print $1; exit }' "$SUMS_FILE")
case "$EXPECTED_SHA256" in ????????*) ;; *) die "SHA256SUMS does not contain $ASSET_NAME." ;; esac
case "$EXPECTED_SHA256" in *[!0123456789abcdefABCDEF]*) die "SHA256SUMS has an invalid hash for $ASSET_NAME." ;; esac
[ "${#EXPECTED_SHA256}" -eq 64 ] || die "SHA256SUMS has an invalid hash length for $ASSET_NAME."
ACTUAL_SHA256=$(sha256_file "$ARCHIVE")
EXPECTED_SHA256=$(printf '%s' "$EXPECTED_SHA256" | tr 'ABCDEF' 'abcdef')
ACTUAL_SHA256=$(printf '%s' "$ACTUAL_SHA256" | tr 'ABCDEF' 'abcdef')
[ "$ACTUAL_SHA256" = "$EXPECTED_SHA256" ] || die "checksum verification failed for $ASSET_NAME."
note 'Release checksum verified.'

command -v tar >/dev/null 2>&1 || die 'tar is required to unpack the release.'
EXTRACT_DIR="$TMP_DIR/package"
mkdir -p "$EXTRACT_DIR"
tar -xzf "$ARCHIVE" -C "$EXTRACT_DIR"
for required_file in io-workbench iowb; do
    [ -f "$EXTRACT_DIR/$required_file" ] || die "release archive is missing required file: $required_file"
done

mkdir -p "$BIN_DIR"
BIN_DIR=$(cd "$BIN_DIR" && pwd -P)
WORKSPACE_ROOT_ABS=$(cd "$WORKSPACE_ROOT" && pwd -P)
HOME_ABS=$(cd "$HOME" && pwd -P)
if [ "$AUTOSTART" -eq 1 ]; then
    if [ ! -d "$CONFIG_DIR" ]; then
        (umask 077 && mkdir -p "$CONFIG_DIR")
        chmod 700 "$CONFIG_DIR" 2>/dev/null || true
    fi
    CONFIG_DIR_ABS=$(cd "$CONFIG_DIR" && pwd -P)
else
    CONFIG_DIR_ABS=$CONFIG_DIR
fi
if [ "$INSTALL_CODEX" -eq 1 ] || [ "$INSTALL_CLAUDE" -eq 1 ] || [ "$INSTALL_GEMINI" -eq 1 ]; then
    if [ ! -d "$NPM_PREFIX" ]; then (umask 077 && mkdir -p "$NPM_PREFIX"); fi
    NPM_PREFIX_ABS=$(cd "$NPM_PREFIX" && pwd -P)
else
    NPM_PREFIX_ABS=$NPM_PREFIX
fi
NPM_BIN_DIR="$NPM_PREFIX_ABS/bin"
NODE_BIN_DIR=''
if command -v node >/dev/null 2>&1; then NODE_BIN_DIR=$(dirname "$(command -v node)"); fi
SERVICE_PATH="$NPM_BIN_DIR:$BIN_DIR:$HOME_ABS/.local/bin"
[ -n "$NODE_BIN_DIR" ] && SERVICE_PATH="$SERVICE_PATH:$NODE_BIN_DIR"
SERVICE_PATH="$SERVICE_PATH:/usr/local/bin:/usr/bin:/bin"

WORKBENCH_BINARY="$BIN_DIR/io-workbench"
install_binary "$EXTRACT_DIR/io-workbench" "$WORKBENCH_BINARY"
install_binary "$EXTRACT_DIR/iowb" "$BIN_DIR/iowb"
note "Installed io-workbench and iowb to $BIN_DIR."
CURRENT_PATH=$(printenv PATH 2>/dev/null || true)
case ":$CURRENT_PATH:" in *":$BIN_DIR:"*) ;; *) warn "$BIN_DIR is not on PATH in this shell."; printf '%s\n' "Add this to your shell profile: export PATH=\"$BIN_DIR:\$PATH\"" ;; esac

if [ "$INSTALL_GATEWAY" -eq 1 ] && ! install_gateway; then
    warn 'io-workbench is installed, but the optional IO Gateway installation failed. Rerun that selection when ready.'
fi
if [ "$INSTALL_CODEX" -eq 1 ] && ! install_provider_cli 'Codex CLI' '@openai/codex' codex; then warn 'Codex CLI installation failed.'; fi
if [ "$INSTALL_CLAUDE" -eq 1 ] && ! install_provider_cli 'Claude Code' '@anthropic-ai/claude-code' claude; then warn 'Claude Code installation failed.'; fi
if [ "$INSTALL_GEMINI" -eq 1 ] && ! install_provider_cli 'Gemini CLI' '@google/gemini-cli' gemini; then warn 'Gemini CLI installation failed.'; fi

if [ "$AUTOSTART" -eq 1 ]; then
    case "$PLATFORM" in
        linux)
            if write_systemd_service; then
                if wait_for_health; then SERVICE_STARTED=1; note "Started io-workbench.service at $(health_url)."
                else warn "the user service was registered but did not become healthy at $(health_url) within 15 seconds."; warn 'Inspect it with: systemctl --user status io-workbench.service'; fi
            elif [ "$SERVICE_WRITTEN" -eq 1 ]; then
                warn 'wrote the user service, but systemd could not enable or start it in this session.'
                warn 'Run: systemctl --user daemon-reload && systemctl --user enable --now io-workbench.service'
            else
                warn 'could not create a systemd user service (common in containers and some SSH sessions).'
            fi
            ;;
        macos)
            if write_launch_agent; then
                if wait_for_health; then SERVICE_STARTED=1; note "Started $LAUNCH_AGENT_LABEL at $(health_url)."
                else warn "the macOS LaunchAgent was registered but did not become healthy at $(health_url) within 15 seconds."; warn "Inspect it with: launchctl print gui/$(id -u)/$LAUNCH_AGENT_LABEL"; fi
            elif [ "$SERVICE_WRITTEN" -eq 1 ]; then
                warn 'wrote the macOS LaunchAgent, but launchctl could not start it in this session.'
                warn "It will load at the next graphical sign-in; or run: launchctl bootstrap gui/$(id -u) $(shell_quote "$(launch_agent_file)")"
            else
                warn 'could not create a macOS LaunchAgent.'
            fi
            ;;
    esac
fi
if [ "$LINGER" -eq 1 ] && [ "$PLATFORM" = linux ]; then
    if command -v loginctl >/dev/null 2>&1 && loginctl enable-linger "$(id -un)" >/dev/null 2>&1; then note 'Enabled systemd user lingering for this account.'
    else warn "could not enable lingering; run: loginctl enable-linger $(id -un)"; fi
fi

if [ "$CONFIGURE_CLIS" -eq 1 ]; then
    if provider_binary codex >/dev/null 2>&1 && ! configure_provider_cli codex 'Codex CLI'; then warn 'Codex CLI setup was not completed.'; fi
    if provider_binary claude >/dev/null 2>&1 && ! configure_provider_cli claude 'Claude Code'; then warn 'Claude Code setup was not completed.'; fi
    if provider_binary gemini >/dev/null 2>&1 && ! configure_provider_cli gemini 'Gemini CLI'; then warn 'Gemini CLI setup was not completed.'; fi
fi
if [ "$CHECK_PROVIDERS" -eq 1 ]; then
    provider_readiness_checks
fi
if [ "$TEST_PROVIDERS" -eq 1 ]; then
    provider_live_tests
fi

printf '\n'
note "Installed $TAG for $TARGET."
note "Runtime data: $CONFIG_DIR_ABS"
note "Workspace authority: $WORKSPACE_ROOT_ABS"
if [ "$SERVICE_STARTED" -eq 1 ]; then
    printf '%s\n' "Open $(health_url | sed 's#/health$##') and complete first-user setup."
else
    printf '%s\n' 'Start a local, authenticated workbench when you are ready:'
    printf '  IO_WORKBENCH_AUTH_REQUIRED=true IO_WORKBENCH_HOST=%s IO_WORKBENCH_PORT=%s \\\n' "$(shell_quote "$HOST")" "$(shell_quote "$PORT")"
    printf '  IO_WORKBENCH_CONFIG_DIR=%s IO_WORKBENCH_WORKSPACE_ROOT=%s \\\n' "$(shell_quote "$CONFIG_DIR_ABS")" "$(shell_quote "$WORKSPACE_ROOT_ABS")"
    printf '  "%s" start\n' "$WORKBENCH_BINARY"
    printf '%s\n' "Then open $(health_url | sed 's#/health$##') and complete first-user setup."
fi
printf '%s\n' 'Authentication is enabled; this installer stored no password, token, OTP secret, or provider credential.'
[ "$INSTALL_GATEWAY" -eq 0 ] || printf '%s\n' 'IO Gateway is separate and was not auto-started. Finish it, then use Settings → IO Gateway to enter its URL and proxy API key.'
[ "$INSTALL_CODEX" -eq 0 ] && [ "$INSTALL_CLAUDE" -eq 0 ] && [ "$INSTALL_GEMINI" -eq 0 ] || printf '%s\n' "Provider CLIs install under $NPM_BIN_DIR; an installer-managed startup item includes that directory."
