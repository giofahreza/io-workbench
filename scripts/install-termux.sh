#!/data/data/com.termux/files/usr/bin/sh
# Install and run io-workbench as a localhost host inside Termux.
#
# The normal path downloads a version-pinned Android/Bionic runtime whose
# digest is embedded in the signed Android app. Do not install a GNU/Linux
# archive on Termux: Android's linker and libc are different. A native source
# build remains available only as an explicit recovery/development choice.

set -u

OFFICIAL_REPOSITORY='https://github.com/giofahreza/io-workbench.git'
OFFICIAL_REF='main'
REPOSITORY=${IO_WORKBENCH_TERMUX_REPOSITORY:-$OFFICIAL_REPOSITORY}
REF=${IO_WORKBENCH_TERMUX_REF:-$OFFICIAL_REF}
PORT=${IO_WORKBENCH_PORT:-8787}
HOST=${IO_WORKBENCH_HOST:-127.0.0.1}
RUNTIME_HOME=${IO_WORKBENCH_TERMUX_HOME:-"$HOME/.local/share/io-workbench"}
SOURCE_DIR=${IO_WORKBENCH_TERMUX_SOURCE_DIR:-"$RUNTIME_HOME/source"}
BIN_DIR=${IO_WORKBENCH_BIN_DIR:-"$HOME/.local/bin"}
CONFIG_DIR=${IO_WORKBENCH_CONFIG_DIR:-"$HOME/.io-workbench"}
WORKSPACE_ROOT=${IO_WORKBENCH_WORKSPACE_ROOT:-"$HOME/projects"}
CARGO_BUILD_JOBS=${IO_WORKBENCH_TERMUX_CARGO_JOBS:-1}
INSTALL_METHOD=${IO_WORKBENCH_TERMUX_INSTALL_METHOD:-prebuilt}
START_AFTER_INSTALL=1
INSTALL_CODEX=0
INSTALL_CLAUDE=0
INSTALL_GEMINI=0
PAIRING_RECORD_PREFIX='__IOWB_LOCAL_TOKEN_V1__='
# The Android client reads this small, fixed-schema file while an external
# Termux command is running. Keep it deliberately separate from the verbose
# bootstrap log: it contains no command output, paths, credentials, or pairing
# token. A new Android request supplies a UUID through IOWB_INSTALL_RUN_ID;
# manual shell runs get a safe process-and-time fallback below.
INSTALL_PROGRESS_PATH=
INSTALL_PROGRESS_RUN_ID=
INSTALL_PROGRESS_SEQUENCE=0
INSTALL_PROGRESS_PHASE=preparing
INSTALL_PROGRESS_STEP=1
INSTALL_PROGRESS_SUCCEEDED=0
INSTALL_PROGRESS_FAILURE_RECORDED=0
# Android receives only concise lifecycle feedback. Its detailed diagnostics
# stay in Termux, while a manual shell invocation retains the helpful tail.
INSTALL_PROGRESS_FROM_ANDROID=0
# The Android APK may bundle a narrowly scoped source overlay while changes are
# awaiting publication upstream. It lives beside this installer so the Android
# handoff can persist and execute both files in Termux private storage. A user
# who deliberately selects another repository/ref is never surprised by the
# official overlay unless they explicitly provide one through the environment.
INSTALLER_DIR=$(CDPATH= cd "$(dirname "$0")" 2>/dev/null && pwd) || INSTALLER_DIR=
TERMUX_SOURCE_OVERLAY_PATH=${IO_WORKBENCH_TERMUX_SOURCE_OVERLAY:-"$INSTALLER_DIR/termux-source-overlay.patch"}
TERMUX_SOURCE_OVERLAY_ENABLED=0
TERMUX_SOURCE_OVERLAY_APPLIED=0
# This small manifest is copied from the signed Android APK. It is the trust
# anchor for a downloaded release binary: URLs are constructed below from the
# fixed official release origin and the exact asset digest is never obtained
# from the network. The raw binary itself must not cross RUN_COMMAND/Binder.
TERMUX_RUNTIME_RELEASE_MANIFEST_PATH="$INSTALLER_DIR/termux-runtime-release-manifest"
PREBUILT_RELEASE_GENERATION=
PREBUILT_RELEASE_TAG=
PREBUILT_RUNTIME_ABI=
PREBUILT_RUNTIME_ASSET=
PREBUILT_RUNTIME_SHA256=
PREBUILT_RUNTIME_SIZE=
PREBUILT_BINARY=
SOURCE_OPTIONS_REQUESTED=0

usage() {
    printf '%s\n' \
        'Usage: install-termux.sh [options]' \
        '' \
        'Install a verified Android/Bionic io-workbench runtime in Termux and create a loopback-only host wrapper.' \
        '' \
        'Options:' \
        '  --repo <url>            Source repository (default: official io-workbench repository).' \
        '  --ref <branch-or-tag>   Git ref to build (default: main).' \
        '  --port <port>            Loopback port (default: 8787).' \
        '  --workspace-root <dir>  Private Termux project parent (default: ~/projects).' \
        '  --prebuilt              Download the verified release runtime (default).' \
        '  --build-from-source     Explicit fallback: clone and build natively in Termux.' \
        '  --no-start              Install but do not start the host.' \
        '  --with-codex            Install the Termux-wrapped Codex CLI after the host build.' \
        '  --with-claude           Install Termux-compatible Claude Code 2.1.112 after the host build.' \
        '  --with-gemini           Install Gemini CLI with Termux ripgrep fallback after the host build.' \
        '  --with-all-clis         Install all three optional provider CLIs.' \
        '  --help                  Show this help.' \
        '' \
        'The script never requests provider credentials. Run each provider login yourself in Termux.' \
        '' \
        'If the verified runtime is unavailable or fails verification, rerun with --build-from-source.'
}

note() {
    printf '%s\n' "io-workbench Termux: $*"
}

fail() {
    message=$1
    status=${2:-1}
    printf '%s\n' "io-workbench Termux: error: $message" >&2
    if [ -n "${LOG_PATH:-}" ] && [ -f "$LOG_PATH" ]; then
        if [ "$INSTALL_PROGRESS_FROM_ANDROID" -eq 1 ]; then
            printf '%s\n' "Detailed diagnostics remain in $LOG_PATH inside Termux." >&2
        else
            printf '%s\n' 'Recent bootstrap log:' >&2
            tail -n 30 "$LOG_PATH" >&2 || true
        fi
    fi
    exit "$status"
}

require_termux() {
    case "${PREFIX:-}" in
        */com.termux/files/usr) ;;
        *)
            printf '%s\n' 'This installer must run from the official Termux app.' >&2
            printf '%s\n' 'Open Termux, then run the documented io-workbench Termux setup command.' >&2
            exit 2
            ;;
    esac
}

valid_port() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
    esac
    [ "$1" -ge 1 ] 2>/dev/null && [ "$1" -le 65535 ] 2>/dev/null
}

valid_build_jobs() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
    esac
    [ "$1" -ge 1 ] 2>/dev/null
}

valid_positive_decimal() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
    esac
    [ "$1" -ge 1 ] 2>/dev/null
}

valid_sha256() {
    candidate_sha256=$1
    [ "${#candidate_sha256}" -eq 64 ] || return 1
    case "$candidate_sha256" in
        *[!0123456789abcdef]*) return 1 ;;
    esac
    return 0
}

valid_release_tag() {
    candidate_tag=$1
    [ "${#candidate_tag}" -ge 2 ] && [ "${#candidate_tag}" -le 96 ] || return 1
    case "$candidate_tag" in
        v[0-9]* ) ;;
        * ) return 1 ;;
    esac
    case "$candidate_tag" in
        *[!ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._-]* ) return 1 ;;
    esac
    return 0
}

valid_release_asset_name() {
    candidate_asset=$1
    [ "${#candidate_asset}" -ge 16 ] && [ "${#candidate_asset}" -le 180 ] || return 1
    case "$candidate_asset" in
        *[!ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._-]* ) return 1 ;;
    esac
    return 0
}

valid_runtime_token() {
    token=$1
    # A local-runtime pairing token is 32 random bytes encoded as hex. Keeping
    # the exact shape here makes the Android callback format unambiguous and
    # prevents a malformed legacy file from silently weakening local auth.
    [ "${#token}" -eq 64 ] || return 1
    case "$token" in
        *[!0123456789abcdefABCDEF]*) return 1 ;;
    esac
    return 0
}

valid_process_id() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
    esac
    [ "$1" -ge 1 ] 2>/dev/null
}

valid_install_progress_run_id() {
    candidate_run_id=$1
    [ -n "$candidate_run_id" ] || return 1
    [ "${#candidate_run_id}" -le 96 ] || return 1
    case "$candidate_run_id" in
        *[!ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._-]*) return 1 ;;
    esac
    return 0
}

new_install_progress_run_id() {
    progress_epoch=$(date +%s 2>/dev/null || true)
    case "$progress_epoch" in
        ''|*[!0-9]*) progress_epoch=0 ;;
    esac
    # This is an opaque correlation ID, not a secret. The PID keeps repeated
    # manual requests distinct even if a device has an unset clock.
    printf 'termux-%s-%s\n' "$progress_epoch" "$$"
}

valid_install_progress_record() {
    progress_state=$1
    progress_phase=$2
    progress_step=$3
    case "$progress_state:$progress_phase:$progress_step" in
        running:preparing:1|running:prerequisites:2|running:source:3|\
        running:downloading:3|running:compiling:4|running:verifying:4|\
        running:configuring:5|running:starting:6|\
        succeeded:complete:6|failed:failed:1|failed:failed:2|\
        failed:failed:3|failed:failed:4|failed:failed:5|failed:failed:6)
            return 0
            ;;
    esac
    return 1
}

write_install_progress() {
    progress_state=$1
    progress_phase=$2
    progress_step=$3
    [ -n "$INSTALL_PROGRESS_PATH" ] || return 1
    valid_install_progress_run_id "$INSTALL_PROGRESS_RUN_ID" || return 1
    valid_install_progress_record "$progress_state" "$progress_phase" "$progress_step" || return 1

    INSTALL_PROGRESS_SEQUENCE=$((INSTALL_PROGRESS_SEQUENCE + 1))
    progress_sequence=$INSTALL_PROGRESS_SEQUENCE
    progress_staged_path="$INSTALL_PROGRESS_PATH.$$.new"
    {
        printf '%s\n' 'version=1'
        printf 'run_id=%s\n' "$INSTALL_PROGRESS_RUN_ID"
        printf 'seq=%s\n' "$progress_sequence"
        printf 'state=%s\n' "$progress_state"
        printf 'phase=%s\n' "$progress_phase"
        printf 'step=%s\n' "$progress_step"
    } >"$progress_staged_path" || {
        rm -f "$progress_staged_path" 2>/dev/null || true
        return 1
    }
    chmod 600 "$progress_staged_path" || {
        rm -f "$progress_staged_path" 2>/dev/null || true
        return 1
    }
    mv -f "$progress_staged_path" "$INSTALL_PROGRESS_PATH" || {
        rm -f "$progress_staged_path" 2>/dev/null || true
        return 1
    }
    INSTALL_PROGRESS_PHASE=$progress_phase
    INSTALL_PROGRESS_STEP=$progress_step
    return 0
}

record_install_progress() {
    write_install_progress running "$1" "$2" || \
        fail 'could not record safe Termux installation progress.'
}

record_install_progress_success() {
    write_install_progress succeeded complete 6 || \
        fail 'could not record successful Termux installation progress.'
    INSTALL_PROGRESS_SUCCEEDED=1
}

record_install_progress_failure() {
    [ "$INSTALL_PROGRESS_SUCCEEDED" -eq 1 ] && return 0
    [ "$INSTALL_PROGRESS_FAILURE_RECORDED" -eq 1 ] && return 0
    if write_install_progress failed failed "$INSTALL_PROGRESS_STEP"; then
        INSTALL_PROGRESS_FAILURE_RECORDED=1
    fi
}

run_logged() {
    "$@" >>"$LOG_PATH" 2>&1
}

run_or_fail() {
    label=$1
    shift
    run_logged "$@"
    status=$?
    if [ "$status" -ne 0 ]; then
        fail "$label failed; open $LOG_PATH in Termux for details." "$status"
    fi
}

termux_runtime_abi() {
    case "$(uname -m)" in
        aarch64|arm64) printf '%s\n' 'aarch64' ;;
        x86_64|amd64) printf '%s\n' 'x86_64' ;;
        *) return 1 ;;
    esac
}

# Read exactly one `key=value` line from the signed-APK manifest. Keeping the
# parser deliberately small means an attacker cannot turn manifest data into
# shell syntax even if they control the Termux filesystem after installation.
read_release_manifest_field() {
    expected_manifest_key=$1
    manifest_line=
    IFS= read -r manifest_line || [ -n "$manifest_line" ] || return 1
    case "$manifest_line" in
        "$expected_manifest_key="*) ;;
        *) return 1 ;;
    esac
    case "$manifest_line" in
        *=*=*) return 1 ;;
    esac
    MANIFEST_FIELD_VALUE=${manifest_line#*=}
    [ -n "$MANIFEST_FIELD_VALUE" ]
}

parse_prebuilt_release_manifest() {
    manifest_path=$TERMUX_RUNTIME_RELEASE_MANIFEST_PATH
    [ -f "$manifest_path" ] && [ ! -L "$manifest_path" ] && [ -r "$manifest_path" ] || return 1
    manifest_size=$(wc -c <"$manifest_path" 2>/dev/null || true)
    set -- $manifest_size
    manifest_size=${1:-}
    valid_positive_decimal "$manifest_size" || return 1
    [ "$manifest_size" -le 2048 ] || return 1

    {
        read_release_manifest_field version || return 1
        manifest_version=$MANIFEST_FIELD_VALUE
        read_release_manifest_field generation || return 1
        manifest_generation=$MANIFEST_FIELD_VALUE
        read_release_manifest_field release_tag || return 1
        manifest_release_tag=$MANIFEST_FIELD_VALUE
        read_release_manifest_field aarch64_asset || return 1
        manifest_aarch64_asset=$MANIFEST_FIELD_VALUE
        read_release_manifest_field aarch64_sha256 || return 1
        manifest_aarch64_sha256=$MANIFEST_FIELD_VALUE
        read_release_manifest_field aarch64_size || return 1
        manifest_aarch64_size=$MANIFEST_FIELD_VALUE
        read_release_manifest_field x86_64_asset || return 1
        manifest_x86_64_asset=$MANIFEST_FIELD_VALUE
        read_release_manifest_field x86_64_sha256 || return 1
        manifest_x86_64_sha256=$MANIFEST_FIELD_VALUE
        read_release_manifest_field x86_64_size || return 1
        manifest_x86_64_size=$MANIFEST_FIELD_VALUE
        manifest_extra_line=
        if IFS= read -r manifest_extra_line || [ -n "$manifest_extra_line" ]; then
            return 1
        fi
        [ "$manifest_version" = 1 ] || return 1
        valid_positive_decimal "$manifest_generation" || return 1
        valid_release_tag "$manifest_release_tag" || return 1
        valid_release_asset_name "$manifest_aarch64_asset" || return 1
        valid_sha256 "$manifest_aarch64_sha256" || return 1
        valid_positive_decimal "$manifest_aarch64_size" || return 1
        valid_release_asset_name "$manifest_x86_64_asset" || return 1
        valid_sha256 "$manifest_x86_64_sha256" || return 1
        valid_positive_decimal "$manifest_x86_64_size" || return 1
        expected_aarch64_asset="io-workbench-$manifest_release_tag-termux-aarch64"
        expected_x86_64_asset="io-workbench-$manifest_release_tag-termux-x86_64"
        [ "$manifest_aarch64_asset" = "$expected_aarch64_asset" ] || return 1
        [ "$manifest_x86_64_asset" = "$expected_x86_64_asset" ] || return 1
        PREBUILT_RELEASE_GENERATION=$manifest_generation
        PREBUILT_RELEASE_TAG=$manifest_release_tag
        case "$PREBUILT_RUNTIME_ABI" in
            aarch64)
                PREBUILT_RUNTIME_ASSET=$manifest_aarch64_asset
                PREBUILT_RUNTIME_SHA256=$manifest_aarch64_sha256
                PREBUILT_RUNTIME_SIZE=$manifest_aarch64_size
                ;;
            x86_64)
                PREBUILT_RUNTIME_ASSET=$manifest_x86_64_asset
                PREBUILT_RUNTIME_SHA256=$manifest_x86_64_sha256
                PREBUILT_RUNTIME_SIZE=$manifest_x86_64_size
                ;;
            *) return 1 ;;
        esac
    } <"$manifest_path" || return 1
    return 0
}

verified_prebuilt_runtime_url() {
    [ -n "$PREBUILT_RELEASE_TAG" ] && [ -n "$PREBUILT_RUNTIME_ASSET" ] || return 1
    printf '%s\n' "https://github.com/giofahreza/io-workbench/releases/download/$PREBUILT_RELEASE_TAG/$PREBUILT_RUNTIME_ASSET"
}

verify_prebuilt_elf_header() {
    candidate_binary=$1
    # Read only the ELF identification and e_machine field. `file` output is
    # descriptive rather than a stable security boundary; these bytes are not.
    set -- $(LC_ALL=C od -An -v -t u1 -N 20 "$candidate_binary" 2>/dev/null || true)
    [ "$#" -eq 20 ] || return 1
    [ "$1" = 127 ] && [ "$2" = 69 ] && [ "$3" = 76 ] && [ "$4" = 70 ] || return 1
    [ "$5" = 2 ] && [ "$6" = 1 ] || return 1
    case "$PREBUILT_RUNTIME_ABI" in
        aarch64) [ "${19}" = 183 ] && [ "${20}" = 0 ] ;;
        x86_64) [ "${19}" = 62 ] && [ "${20}" = 0 ] ;;
        *) return 1 ;;
    esac
}

download_verified_prebuilt_runtime() {
    PREBUILT_RUNTIME_ABI=$(termux_runtime_abi) || \
        fail "no verified Termux runtime is published for $(uname -m). Rerun with --build-from-source."
    if ! parse_prebuilt_release_manifest; then
        fail 'this APK does not contain a published verified Termux runtime. Install a signed release APK or rerun with --build-from-source.'
    fi
    prebuilt_url=$(verified_prebuilt_runtime_url) || fail 'the signed Termux runtime manifest is incomplete.'
    PREBUILT_BINARY="$RUNTIME_HOME/.io-workbench-prebuilt.$$.download"
    rm -f "$PREBUILT_BINARY" || fail 'could not clear the staged verified runtime download.'

    record_install_progress downloading 3
    note "downloading verified $PREBUILT_RUNTIME_ABI Android/Bionic runtime from release $PREBUILT_RELEASE_TAG."
    if ! curl --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --location \
        --silent --show-error --connect-timeout 20 --retry 2 --max-filesize 100000000 \
        --output "$PREBUILT_BINARY" "$prebuilt_url" >>"$LOG_PATH" 2>&1; then
        rm -f "$PREBUILT_BINARY" || true
        fail 'could not download the verified Termux runtime. Check connectivity, then retry or choose --build-from-source.'
    fi

    record_install_progress verifying 4
    staged_size=$(wc -c <"$PREBUILT_BINARY" 2>/dev/null || true)
    set -- $staged_size
    staged_size=${1:-}
    if ! valid_positive_decimal "$staged_size" || [ "$staged_size" != "$PREBUILT_RUNTIME_SIZE" ]; then
        rm -f "$PREBUILT_BINARY" || true
        fail 'the downloaded Termux runtime size does not match the signed APK manifest.'
    fi
    checksum_output=$(openssl dgst -sha256 -r "$PREBUILT_BINARY" 2>>"$LOG_PATH") || {
        rm -f "$PREBUILT_BINARY" || true
        fail 'could not checksum the downloaded Termux runtime.'
    }
    set -- $checksum_output
    calculated_sha256=${1:-}
    if ! valid_sha256 "$calculated_sha256" || [ "$calculated_sha256" != "$PREBUILT_RUNTIME_SHA256" ]; then
        rm -f "$PREBUILT_BINARY" || true
        fail 'the downloaded Termux runtime checksum does not match the signed APK manifest.'
    fi
    if ! verify_prebuilt_elf_header "$PREBUILT_BINARY"; then
        rm -f "$PREBUILT_BINARY" || true
        fail 'the downloaded runtime has the wrong Android/Bionic executable format for this Termux device.'
    fi
    chmod 700 "$PREBUILT_BINARY" || {
        rm -f "$PREBUILT_BINARY" || true
        fail 'could not mark the verified Termux runtime executable.'
    }
    if ! "$PREBUILT_BINARY" --version >>"$LOG_PATH" 2>&1; then
        rm -f "$PREBUILT_BINARY" || true
        fail 'the verified runtime could not start on this Termux device. Rerun with --build-from-source.'
    fi
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --repo)
            [ "$#" -ge 2 ] || fail '--repo needs a URL.' 2
            REPOSITORY=$2
            SOURCE_OPTIONS_REQUESTED=1
            shift 2
            ;;
        --ref)
            [ "$#" -ge 2 ] || fail '--ref needs a branch or tag.' 2
            REF=$2
            SOURCE_OPTIONS_REQUESTED=1
            shift 2
            ;;
        --port)
            [ "$#" -ge 2 ] || fail '--port needs a value.' 2
            PORT=$2
            shift 2
            ;;
        --workspace-root)
            [ "$#" -ge 2 ] || fail '--workspace-root needs a directory.' 2
            WORKSPACE_ROOT=$2
            shift 2
            ;;
        --prebuilt)
            INSTALL_METHOD=prebuilt
            shift
            ;;
        --build-from-source|--source)
            INSTALL_METHOD=source
            shift
            ;;
        --no-start)
            START_AFTER_INSTALL=0
            shift
            ;;
        --with-codex)
            INSTALL_CODEX=1
            shift
            ;;
        --with-claude)
            INSTALL_CLAUDE=1
            shift
            ;;
        --with-gemini)
            INSTALL_GEMINI=1
            shift
            ;;
        --with-all-clis)
            INSTALL_CODEX=1
            INSTALL_CLAUDE=1
            INSTALL_GEMINI=1
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            fail "unknown option: $1" 2
            ;;
    esac
done

case "$INSTALL_METHOD" in
    prebuilt|source) ;;
    *) fail 'IO_WORKBENCH_TERMUX_INSTALL_METHOD must be prebuilt or source.' 2 ;;
esac
if [ "$SOURCE_OPTIONS_REQUESTED" -eq 1 ] && [ "$INSTALL_METHOD" != source ]; then
    fail '--repo and --ref are source-build options. Add --build-from-source to use them.' 2
fi

if [ "$INSTALL_METHOD" = source ] && [ -n "${IO_WORKBENCH_TERMUX_SOURCE_OVERLAY:-}" ]; then
    [ -f "$TERMUX_SOURCE_OVERLAY_PATH" ] || \
        fail "the requested Termux source overlay does not exist: $TERMUX_SOURCE_OVERLAY_PATH" 2
    TERMUX_SOURCE_OVERLAY_ENABLED=1
elif [ "$INSTALL_METHOD" = source ] && [ "$REPOSITORY" = "$OFFICIAL_REPOSITORY" ] && [ "$REF" = "$OFFICIAL_REF" ] && \
    [ -f "$TERMUX_SOURCE_OVERLAY_PATH" ]; then
    TERMUX_SOURCE_OVERLAY_ENABLED=1
fi

require_termux
valid_port "$PORT" || fail 'port must be a whole number from 1 through 65535.' 2
valid_build_jobs "$CARGO_BUILD_JOBS" || fail 'IO_WORKBENCH_TERMUX_CARGO_JOBS must be a positive whole number.' 2

# Keep the package and child process environment native to Termux. In
# particular, do not clear LD_PRELOAD: termux-exec relies on it on modern Android.
export PATH="$PREFIX/bin:$BIN_DIR:$PATH"
export HOME
umask 077

LOG_DIR="$CONFIG_DIR/logs"
LOG_PATH="$LOG_DIR/termux-bootstrap.log"
RUNTIME_TOKEN_PATH="$CONFIG_DIR/termux-runtime-token"
mkdir -p "$LOG_DIR" "$RUNTIME_HOME" "$BIN_DIR" "$CONFIG_DIR" "$WORKSPACE_ROOT" || \
    fail 'could not create the private runtime directories.'
: >"$LOG_PATH" || fail "could not write $LOG_PATH"

INSTALL_PROGRESS_PATH="$CONFIG_DIR/termux-install-progress"
requested_install_progress_run_id=${IOWB_INSTALL_RUN_ID:-}
if valid_install_progress_run_id "$requested_install_progress_run_id"; then
    INSTALL_PROGRESS_RUN_ID=$requested_install_progress_run_id
    INSTALL_PROGRESS_FROM_ANDROID=1
else
    INSTALL_PROGRESS_RUN_ID=$(new_install_progress_run_id)
fi
valid_install_progress_run_id "$INSTALL_PROGRESS_RUN_ID" || \
    fail 'could not create a safe Termux installation progress identifier.'

# A Termux RunCommand request can be retried while its earlier install is
# still building. Both invocations would otherwise rewrite the same source
# checkout, temporary Cargo manifest, binary, and runtime settings. `mkdir`
# is atomic on Termux's private filesystem, so use it as a small lock without
# introducing another package dependency.
INSTALL_LOCK_DIR="$RUNTIME_HOME/.termux-install.lock"
INSTALL_LOCK_HELD=0

release_install_lock() {
    [ "$INSTALL_LOCK_HELD" -eq 1 ] || return 0
    rm -f "$INSTALL_LOCK_DIR/pid" 2>/dev/null || true
    if ! rmdir "$INSTALL_LOCK_DIR" 2>/dev/null; then
        printf '%s\n' "io-workbench Termux: warning: could not remove install lock $INSTALL_LOCK_DIR" >&2
    fi
    INSTALL_LOCK_HELD=0
}

acquire_install_lock() {
    if mkdir "$INSTALL_LOCK_DIR" 2>/dev/null; then
        printf '%s\n' "$$" >"$INSTALL_LOCK_DIR/pid" || {
            rmdir "$INSTALL_LOCK_DIR" 2>/dev/null || true
            fail 'could not record the Termux installer lock owner.'
        }
        INSTALL_LOCK_HELD=1
        return 0
    fi

    lock_owner=
    if [ -r "$INSTALL_LOCK_DIR/pid" ]; then
        IFS= read -r lock_owner <"$INSTALL_LOCK_DIR/pid" || true
    fi
    if valid_process_id "$lock_owner" && kill -0 "$lock_owner" 2>/dev/null; then
        fail "another io-workbench Termux install is already running (PID $lock_owner). Wait for it to finish before retrying."
    fi

    # The previous shell is gone, so a stale lock can be reclaimed. Remove
    # only the known PID entry and require the directory to be otherwise empty
    # rather than recursively deleting a path in the user's runtime directory.
    rm -f "$INSTALL_LOCK_DIR/pid" 2>/dev/null || \
        fail "could not inspect the stale Termux installer lock at $INSTALL_LOCK_DIR."
    rmdir "$INSTALL_LOCK_DIR" 2>/dev/null || \
        fail "a stale Termux installer lock remains at $INSTALL_LOCK_DIR; inspect it in Termux before retrying."
    mkdir "$INSTALL_LOCK_DIR" || fail "could not acquire the Termux installer lock at $INSTALL_LOCK_DIR."
    printf '%s\n' "$$" >"$INSTALL_LOCK_DIR/pid" || {
        rmdir "$INSTALL_LOCK_DIR" 2>/dev/null || true
        fail 'could not record the reclaimed Termux installer lock owner.'
    }
    INSTALL_LOCK_HELD=1
}

acquire_install_lock
trap 'record_install_progress_failure; release_install_lock' EXIT
trap 'release_install_lock; exit 130' HUP INT TERM

record_install_progress preparing 1

if [ "$INSTALL_METHOD" = prebuilt ]; then
    note 'preparing the verified Android/Bionic runtime install; io-workbench Mobile can read the private installation progress record.'
    # Fail before updating packages when a development APK has no signed
    # release manifest. This is deliberately not an implicit Cargo fallback.
    PREBUILT_RUNTIME_ABI=$(termux_runtime_abi) || \
        fail "no verified Termux runtime is published for $(uname -m). Rerun with --build-from-source."
    parse_prebuilt_release_manifest || \
        fail 'this APK does not contain a published verified Termux runtime. Install a signed release APK or rerun with --build-from-source.'
else
    note 'preparing the explicit native source build; io-workbench Mobile can read the private installation progress record.'
fi
# `pkg update` checks mirrors by starting the standalone curl binary first.
# A partially completed Termux upgrade can leave curl linked against a newer
# libcurl than the installed OpenSSL, which makes that mirror check impossible.
# Source compilation may repair that package set with a full upgrade. The
# verified-runtime route intentionally fails with clear guidance instead: a
# full upgrade can pull Node's native toolchain even though this route never
# builds Rust locally.
if ! curl --version >/dev/null 2>&1; then
    if [ "$INSTALL_METHOD" = source ]; then
        note 'repairing an incomplete Termux package upgrade before checking mirrors.'
        record_install_progress prerequisites 2
        run_or_fail 'Termux package recovery' env DEBIAN_FRONTEND=noninteractive \
            apt-get -o Dpkg::Options::=--force-confold -y full-upgrade
    else
        fail 'Termux curl cannot start, so this verified install will not run a broad package upgrade. Repair Termux in its own app, then retry.'
    fi
fi
record_install_progress prerequisites 2
run_or_fail 'Termux package index update' pkg update -y
if [ "$INSTALL_METHOD" = source ]; then
    run_or_fail 'Termux package upgrade' env DEBIAN_FRONTEND=noninteractive \
        apt-get -o Dpkg::Options::=--force-confold -y full-upgrade
fi
# Termux splits the `openssl` binary into `openssl-tool`; the library package
# alone is not enough for the private pairing-token generation or verified
# runtime checksum below. `coreutils` provides the stable `od` used to check
# the signed binary's ELF machine field before it can run.
# Python, its venv seed wheels, and pip are normal local-development tools
# (including for Django), not provider dependencies. Install with
# --no-install-recommends below: current Termux python-pip recommends Clang,
# Make, and pkg-config even though the verified io-workbench runtime and a
# pure-Python Django project do not need a local compiler. Node/PRoot are
# also optional runtime dependencies, so keep them out of the verified-runtime
# path unless the user explicitly asks for one of the provider CLIs.
runtime_packages='bash git curl ca-certificates openssl openssl-tool coreutils python python-pip python-ensurepip-wheels ripgrep sqlite'
if [ "$INSTALL_CODEX" -eq 1 ] || [ "$INSTALL_CLAUDE" -eq 1 ] || [ "$INSTALL_GEMINI" -eq 1 ]; then
    # `nodejs-lts` recommends rather than depends on `npm` in Termux. The
    # normal verified-runtime route intentionally uses --no-install-recommends,
    # but an explicitly requested provider CLI must be able to run its
    # subsequent `npm install --global` step.
    runtime_packages="$runtime_packages nodejs-lts npm"
fi
if [ "$INSTALL_CODEX" -eq 1 ]; then
    runtime_packages="$runtime_packages proot"
fi
if [ "$INSTALL_METHOD" = source ]; then
    runtime_packages="$runtime_packages rust clang make pkg-config"
fi
# A base Termux install can carry locally modified shell profiles. Tell dpkg
# to retain those files rather than opening an unreadable conffile prompt in a
# background RUN_COMMAND task. `--no-install-recommends` is equally important:
# python-pip otherwise recommends the compiler packages that the verified
# runtime deliberately avoids.
run_or_fail 'Termux runtime prerequisite installation' env DEBIAN_FRONTEND=noninteractive \
    apt-get -o Dpkg::Options::=--force-confold -y --no-install-recommends install $runtime_packages

# Android loopback is shared between applications.  A plain first-user setup
# page would let another local app race the owner and claim the server before
# io-workbench Mobile connects.  Generate a high-entropy Termux-private bearer
# token for a fresh runtime instead. Existing password-protected runtimes are
# deliberately left alone, so an upgrade never invalidates their login. A
# previous token-authenticated runtime contains only the built-in `local`
# user, which is safe to repair with a newly generated token if its token file
# was lost; do not mistake that account for a password user.
runtime_has_password_user() {
    database_path="$CONFIG_DIR/io-workbench.db"
    [ -e "$database_path" ] || return 1
    [ -f "$database_path" ] || fail "the local runtime database path is not a regular file: $database_path"
    [ -s "$database_path" ] || return 1
    user_count=$(sqlite3 "$database_path" "SELECT COUNT(*) FROM users WHERE id != 'local';" 2>/dev/null) || \
        fail "could not inspect the existing local runtime database at $database_path; preserving it unchanged."
    case "$user_count" in
        ''|*[!0-9]*) fail "the existing local runtime database returned an invalid user count; preserving it unchanged." ;;
    esac
    [ "$user_count" -gt 0 ]
}

ensure_runtime_token() {
    if [ -e "$RUNTIME_TOKEN_PATH" ] || [ -L "$RUNTIME_TOKEN_PATH" ]; then
        [ -f "$RUNTIME_TOKEN_PATH" ] && [ ! -L "$RUNTIME_TOKEN_PATH" ] || \
            fail "the existing local runtime token at $RUNTIME_TOKEN_PATH is not a regular file; preserve it and replace it manually."
        IFS= read -r existing_token <"$RUNTIME_TOKEN_PATH" || true
        valid_runtime_token "$existing_token" || \
            fail "the existing local runtime token at $RUNTIME_TOKEN_PATH is invalid; preserve it and replace it manually."
        chmod 600 "$RUNTIME_TOKEN_PATH" || \
            fail "could not protect the existing local runtime token at $RUNTIME_TOKEN_PATH."
        return 0
    fi
    if runtime_has_password_user; then
        note 'preserving the existing password-based local runtime authentication.'
        return 0
    fi

    runtime_token=$(openssl rand -hex 32) || fail 'could not generate the local runtime token.'
    valid_runtime_token "$runtime_token" || fail 'generated an invalid local runtime token.'
    staged_runtime_token="$CONFIG_DIR/.termux-runtime-token.$$.new"
    printf '%s\n' "$runtime_token" >"$staged_runtime_token" || \
        fail "could not write the local runtime token at $RUNTIME_TOKEN_PATH."
    chmod 600 "$staged_runtime_token" || {
        rm -f "$staged_runtime_token" || true
        fail 'could not protect the local runtime token.'
    }
    mv -f "$staged_runtime_token" "$RUNTIME_TOKEN_PATH" || {
        rm -f "$staged_runtime_token" || true
        fail "could not install the local runtime token at $RUNTIME_TOKEN_PATH."
    }
    note 'created a private local-runtime token; the Android app receives it only through the explicit Termux result callback.'
}

ensure_runtime_token

emit_runtime_pairing_record() {
    # No token is expected when preserving a pre-existing password runtime.
    # In that case Android deliberately falls back to its ordinary password
    # connection flow rather than treating an empty marker as authentication.
    if [ ! -e "$RUNTIME_TOKEN_PATH" ] && [ ! -L "$RUNTIME_TOKEN_PATH" ]; then
        return 1
    fi
    [ -f "$RUNTIME_TOKEN_PATH" ] && [ ! -L "$RUNTIME_TOKEN_PATH" ] && [ -r "$RUNTIME_TOKEN_PATH" ] || \
        fail "the local runtime token changed into an unreadable non-regular file: $RUNTIME_TOKEN_PATH"
    IFS= read -r pairing_token <"$RUNTIME_TOKEN_PATH" || true
    valid_runtime_token "$pairing_token" || \
        fail "the local runtime token became invalid: $RUNTIME_TOKEN_PATH"
    # This is intentionally the sole token-bearing stdout output. The Android
    # RunCommand receiver accepts the exact record only after a successful
    # install/pair command, removes it from diagnostics, and stores it in its
    # local secret store.
    printf '%s%s;port=%s\n' "$PAIRING_RECORD_PREFIX" "$pairing_token" "$PORT"
}

if [ "$INSTALL_METHOD" = source ]; then
record_install_progress source 3

if [ -e "$SOURCE_DIR" ] && [ ! -d "$SOURCE_DIR/.git" ]; then
    fail "$SOURCE_DIR already exists and is not an io-workbench Git checkout; move it aside manually before retrying."
fi

RAG_WORKSPACE_MEMBER='^[[:space:]]*"rag",[[:space:]]*$'
recover_interrupted_termux_build() {
    [ -f "$SOURCE_DIR/Cargo.toml" ] || return 0
    for working in "$SOURCE_DIR"/.Cargo.toml.termux-build.*.working; do
        [ -f "$working" ] || continue
        build_id=${working##*.Cargo.toml.termux-build.}
        build_id=${build_id%.working}
        backup="$RUNTIME_HOME/Cargo.toml.termux-build.$build_id.backup"
        if [ -f "$backup" ]; then
            note 'discarding the incomplete temporary manifest from an interrupted Termux build.'
            rm -f "$working" || fail "could not remove $working from the interrupted Termux build."
        else
            fail "an interrupted Termux build left $working without its recovery backup; preserve it and remove it manually before retrying."
        fi
    done
    for backup in "$RUNTIME_HOME"/Cargo.toml.termux-build.*.backup; do
        [ -f "$backup" ] || continue
        if ! grep -q "$RAG_WORKSPACE_MEMBER" "$SOURCE_DIR/Cargo.toml" && \
            grep -q "$RAG_WORKSPACE_MEMBER" "$backup"; then
            if [ "$(sed '/^[[:space:]]*"rag",[[:space:]]*$/d' "$backup")" = "$(cat "$SOURCE_DIR/Cargo.toml")" ]; then
                note 'restoring the manifest from an interrupted Termux build.'
                mv "$backup" "$SOURCE_DIR/Cargo.toml" || fail 'could not restore the source manifest from the interrupted build.'
            else
                fail "an interrupted Termux build left $SOURCE_DIR/Cargo.toml changed; preserve your work and restore $backup manually before retrying."
            fi
        fi
    done
    for backup in "$RUNTIME_HOME"/Cargo.lock.termux-build.*.backup; do
        [ -f "$backup" ] || continue
        # A lock backup exists only while the paired manifest was temporarily
        # pruned for a Termux build. At this point the installer-owned checkout
        # has already restored its manifest, so put the exact lock file back
        # before Cargo sees the full workspace again.
        if [ -f "$SOURCE_DIR/Cargo.lock" ]; then
            mv "$backup" "$SOURCE_DIR/Cargo.lock" || \
                fail 'could not restore the Cargo lock file from the interrupted Termux build.'
        else
            fail "an interrupted Termux build left $SOURCE_DIR/Cargo.lock missing; restore $backup manually before retrying."
        fi
    done
}
recover_interrupted_termux_build

recover_interrupted_termux_source_overlay() {
    [ "$TERMUX_SOURCE_OVERLAY_ENABLED" -eq 1 ] || return 0
    [ -d "$SOURCE_DIR/.git" ] || return 0
    if git -C "$SOURCE_DIR" diff --quiet && git -C "$SOURCE_DIR" diff --cached --quiet; then
        return 0
    fi
    # An interrupted native build can leave only our uncommitted overlay in
    # the installer-owned checkout. Reverse it only when Git proves that the
    # exact overlay is present; any other user change remains protected by the
    # ordinary dirty-check below.
    if git -C "$SOURCE_DIR" diff --cached --quiet && \
        git -C "$SOURCE_DIR" apply --reverse --check "$TERMUX_SOURCE_OVERLAY_PATH"; then
        note 'restoring an interrupted Termux source compatibility overlay.'
        git -C "$SOURCE_DIR" apply --reverse "$TERMUX_SOURCE_OVERLAY_PATH" || \
            fail 'could not restore the interrupted Termux source compatibility overlay.'
    fi
}

restore_termux_source_overlay() {
    [ "$TERMUX_SOURCE_OVERLAY_APPLIED" -eq 1 ] || return 0
    if git -C "$SOURCE_DIR" apply --reverse --check "$TERMUX_SOURCE_OVERLAY_PATH" && \
        git -C "$SOURCE_DIR" apply --reverse "$TERMUX_SOURCE_OVERLAY_PATH"; then
        TERMUX_SOURCE_OVERLAY_APPLIED=0
        return 0
    fi
    printf '%s\n' 'io-workbench Termux: error: could not restore the temporary source compatibility overlay.' >&2
    return 1
}

apply_termux_source_overlay() {
    [ "$TERMUX_SOURCE_OVERLAY_ENABLED" -eq 1 ] || return 0
    if ! git -C "$SOURCE_DIR" apply --check "$TERMUX_SOURCE_OVERLAY_PATH"; then
        fail 'the bundled Termux source compatibility overlay no longer matches the selected source. Update io-workbench Mobile or use a matching published source revision.'
    fi
    note 'applying the bundled Termux source compatibility overlay for this native build.'
    run_or_fail 'Termux source compatibility overlay' git -C "$SOURCE_DIR" apply "$TERMUX_SOURCE_OVERLAY_PATH"
    TERMUX_SOURCE_OVERLAY_APPLIED=1
}

recover_interrupted_termux_source_overlay
# From this point an overlay can be applied before the Cargo-manifest recovery
# helpers are defined. Keep the source checkout recoverable even if Android
# interrupts the installer in that short interval; the later trap adds the
# manifest restoration as well.
trap 'record_install_progress_failure; restore_termux_source_overlay; release_install_lock' EXIT
trap 'restore_termux_source_overlay; release_install_lock; exit 130' HUP INT TERM

if [ ! -d "$SOURCE_DIR/.git" ]; then
    note "cloning $REF from the configured source repository."
    run_or_fail 'source clone' git clone --depth 1 --branch "$REF" "$REPOSITORY" "$SOURCE_DIR"
else
    if ! git -C "$SOURCE_DIR" diff --quiet || ! git -C "$SOURCE_DIR" diff --cached --quiet; then
        fail "$SOURCE_DIR has local changes; preserving them. Review or move that checkout before updating."
    fi
    note "updating the existing $REF source checkout."
    run_or_fail 'source update' git -C "$SOURCE_DIR" fetch --depth 1 origin "$REF"
    run_or_fail 'source checkout' git -C "$SOURCE_DIR" checkout --detach FETCH_HEAD
fi

apply_termux_source_overlay

# The native RAG implementation and mobile-app source are private Git
# submodules. The localhost runtime intentionally disables RAG, but Cargo
# still reads every workspace member before selecting iowb-cli. Temporarily
# omit the unavailable RAG member only while building, then restore the exact
# checked-out manifest so retries and ordinary Git updates remain clean.
TERMUX_BUILD_MANIFEST=
TERMUX_BUILD_MANIFEST_BACKUP=
TERMUX_BUILD_MANIFEST_WORKING=
TERMUX_BUILD_LOCKFILE=
TERMUX_BUILD_LOCKFILE_BACKUP=
restore_termux_workspace_manifest() {
    if [ -n "$TERMUX_BUILD_MANIFEST_WORKING" ] && [ -f "$TERMUX_BUILD_MANIFEST_WORKING" ]; then
        if rm -f "$TERMUX_BUILD_MANIFEST_WORKING"; then
            TERMUX_BUILD_MANIFEST_WORKING=
        else
            printf '%s\n' "io-workbench Termux: error: could not remove $TERMUX_BUILD_MANIFEST_WORKING" >&2
            return 1
        fi
    fi
    if [ -n "$TERMUX_BUILD_LOCKFILE_BACKUP" ] && [ -f "$TERMUX_BUILD_LOCKFILE_BACKUP" ]; then
        if mv "$TERMUX_BUILD_LOCKFILE_BACKUP" "$TERMUX_BUILD_LOCKFILE"; then
            TERMUX_BUILD_LOCKFILE_BACKUP=
        else
            printf '%s\n' "io-workbench Termux: error: could not restore $TERMUX_BUILD_LOCKFILE" >&2
            return 1
        fi
    fi
    if [ -n "$TERMUX_BUILD_MANIFEST_BACKUP" ] && [ -f "$TERMUX_BUILD_MANIFEST_BACKUP" ]; then
        if mv "$TERMUX_BUILD_MANIFEST_BACKUP" "$TERMUX_BUILD_MANIFEST"; then
            TERMUX_BUILD_MANIFEST_BACKUP=
        else
            printf '%s\n' "io-workbench Termux: error: could not restore $TERMUX_BUILD_MANIFEST" >&2
            return 1
        fi
    fi
    return 0
}
trap 'record_install_progress_failure; restore_termux_workspace_manifest; restore_termux_source_overlay; release_install_lock' EXIT
trap 'restore_termux_workspace_manifest; restore_termux_source_overlay; release_install_lock; exit 130' HUP INT TERM

build_termux_binary() {
    TERMUX_BUILD_MANIFEST="$SOURCE_DIR/Cargo.toml"
    TERMUX_BUILD_MANIFEST_BACKUP="$RUNTIME_HOME/Cargo.toml.termux-build.$$.backup"
    TERMUX_BUILD_MANIFEST_WORKING="$SOURCE_DIR/.Cargo.toml.termux-build.$$.working"
    TERMUX_BUILD_LOCKFILE="$SOURCE_DIR/Cargo.lock"
    TERMUX_BUILD_LOCKFILE_BACKUP="$RUNTIME_HOME/Cargo.lock.termux-build.$$.backup"
    cp "$TERMUX_BUILD_MANIFEST" "$TERMUX_BUILD_MANIFEST_BACKUP" || return 1
    cp "$TERMUX_BUILD_LOCKFILE" "$TERMUX_BUILD_LOCKFILE_BACKUP" || return 1
    # Write alongside the manifest and atomically replace it only once the
    # complete pruned form is ready. An Android process kill therefore leaves
    # either the original or an exactly recoverable manifest.
    if ! sed '/^[[:space:]]*"rag",[[:space:]]*$/d' "$TERMUX_BUILD_MANIFEST_BACKUP" >"$TERMUX_BUILD_MANIFEST_WORKING"; then
        restore_termux_workspace_manifest
        return 1
    fi
    if ! mv "$TERMUX_BUILD_MANIFEST_WORKING" "$TERMUX_BUILD_MANIFEST"; then
        restore_termux_workspace_manifest
        return 1
    fi
    TERMUX_BUILD_MANIFEST_WORKING=
    if grep -q "$RAG_WORKSPACE_MEMBER" "$TERMUX_BUILD_MANIFEST"; then
        printf '%s\n' 'could not omit the RAG workspace member from Cargo.toml.' >&2
        restore_termux_workspace_manifest
        return 1
    fi
    # Pruning one workspace member changes Cargo's generated workspace
    # metadata in Cargo.lock, so --locked must not be used for this temporary
    # build. The exact checked-out lock file is restored immediately below.
    if ! (cd "$SOURCE_DIR" && cargo build --release -j "$CARGO_BUILD_JOBS" -p iowb-cli --bin io-workbench); then
        return 1
    fi
    restore_termux_workspace_manifest
}

record_install_progress compiling 4
note 'RAG is unavailable in the local Termux runtime and remains disabled.'
note "building with $CARGO_BUILD_JOBS Cargo job(s); raise IO_WORKBENCH_TERMUX_CARGO_JOBS only on devices with ample RAM."
note 'building the Android/Bionic release binary; this can take several minutes on a phone.'
run_or_fail 'Rust release build' build_termux_binary

SOURCE_BINARY="$SOURCE_DIR/target/release/io-workbench"
[ -f "$SOURCE_BINARY" ] || fail 'the Rust build reported success but did not produce io-workbench.'
else
    download_verified_prebuilt_runtime
    SOURCE_BINARY=$PREBUILT_BINARY
fi

record_install_progress configuring 5
# Android refuses to truncate an executable while the running Termux host has
# it mapped (ETXTBSY).  Copy to a fresh sibling and atomically rename it into
# place instead: the already-running host keeps its old inode, while the next
# start uses the complete new build. This also avoids ever exposing a partial
# executable if the installer is interrupted.
INSTALLED_BINARY="$BIN_DIR/io-workbench"
STAGED_BINARY="$BIN_DIR/.io-workbench.$$.new"
cp "$SOURCE_BINARY" "$STAGED_BINARY" || fail 'could not stage the io-workbench binary.'
chmod 700 "$STAGED_BINARY" || {
    rm -f "$STAGED_BINARY" || true
    fail 'could not mark the staged io-workbench binary executable.'
}
mv -f "$STAGED_BINARY" "$INSTALLED_BINARY" || {
    rm -f "$STAGED_BINARY" || true
    fail 'could not atomically install the io-workbench binary.'
}
# The verified download has now been copied into the owned launcher path. Do
# not retain a second full server binary in the runtime directory; it is only
# a staged transport artifact and can be downloaded again during an explicit
# repair. A cleanup failure is non-fatal because the installed binary is
# already complete and executable, but report it in the private log.
if [ "$INSTALL_METHOD" = prebuilt ] && [ -n "$PREBUILT_BINARY" ]; then
    if ! rm -f "$PREBUILT_BINARY"; then
        note "could not remove the staged verified runtime download: $PREBUILT_BINARY"
    fi
    PREBUILT_BINARY=
fi

# Persist the supported installer choices without evaluating user input in the
# launcher. This makes --port and --workspace-root survive later Start, Check,
# pairing, and manual wrapper invocations.
RUNTIME_PORT_PATH="$CONFIG_DIR/termux-runtime-port"
RUNTIME_WORKSPACE_ROOT_PATH="$CONFIG_DIR/termux-runtime-workspace-root"
RUNTIME_TOKEN_PATH="$CONFIG_DIR/termux-runtime-token"
RUNTIME_RELEASE_PATH="$CONFIG_DIR/termux-runtime-release"
write_private_runtime_setting() {
    setting_path=$1
    setting_value=$2
    staged_setting="$setting_path.$$.new"
    printf '%s\n' "$setting_value" >"$staged_setting" || \
        fail "could not stage the local runtime setting at $setting_path."
    chmod 600 "$staged_setting" || {
        rm -f "$staged_setting" || true
        fail "could not protect the staged local runtime setting at $setting_path."
    }
    mv -f "$staged_setting" "$setting_path" || {
        rm -f "$staged_setting" || true
        fail "could not save the local runtime setting at $setting_path."
    }
}

write_private_runtime_setting "$RUNTIME_PORT_PATH" "$PORT"
write_private_runtime_setting "$RUNTIME_WORKSPACE_ROOT_PATH" "$WORKSPACE_ROOT"
if [ "$INSTALL_METHOD" = prebuilt ]; then
    write_private_runtime_setting "$RUNTIME_RELEASE_PATH" "version=1
method=prebuilt
generation=$PREBUILT_RELEASE_GENERATION
release_tag=$PREBUILT_RELEASE_TAG
abi=$PREBUILT_RUNTIME_ABI
sha256=$PREBUILT_RUNTIME_SHA256"
else
    write_private_runtime_setting "$RUNTIME_RELEASE_PATH" "version=1
method=source"
fi

WRAPPER_STAGED="$BIN_DIR/.io-workbench-local.$$.new"
cat >"$WRAPPER_STAGED" <<'WRAPPER'
#!/data/data/com.termux/files/usr/bin/sh
set -u

TERMUX_PREFIX=${PREFIX:-/data/data/com.termux/files/usr}
TERMUX_HOME=${HOME:-/data/data/com.termux/files/home}
BIN_DIR=${IO_WORKBENCH_BIN_DIR:-"$TERMUX_HOME/.local/bin"}
CONFIG_DIR=${IO_WORKBENCH_CONFIG_DIR:-"$TERMUX_HOME/.io-workbench"}
RUNTIME_HOME=${IO_WORKBENCH_TERMUX_HOME:-"$TERMUX_HOME/.local/share/io-workbench"}
INSTALL_LOCK_DIR="$RUNTIME_HOME/.termux-install.lock"
RUNTIME_PORT_PATH="$CONFIG_DIR/termux-runtime-port"
RUNTIME_WORKSPACE_ROOT_PATH="$CONFIG_DIR/termux-runtime-workspace-root"
RUNTIME_TOKEN_PATH="$CONFIG_DIR/termux-runtime-token"
PAIRING_RECORD_PREFIX='__IOWB_LOCAL_TOKEN_V1__='

valid_port() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
    esac
    [ "$1" -ge 1 ] 2>/dev/null && [ "$1" -le 65535 ] 2>/dev/null
}

valid_runtime_token() {
    token=$1
    [ "${#token}" -eq 64 ] || return 1
    case "$token" in
        *[!0123456789abcdefABCDEF]*) return 1 ;;
    esac
    return 0
}

valid_process_id() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
    esac
    [ "$1" -ge 1 ] 2>/dev/null
}

# The installer and uninstaller share this small ownership lock.  Stopping a
# host remains allowed while it is held, but starting one must wait until the
# lifecycle owner finishes.  The installer itself is the sole exception: its
# direct child wrapper sees the installer's PID as PPID, matching the lock's
# recorded owner while it performs the final verified restart.
ensure_runtime_start_allowed() {
    [ -e "$INSTALL_LOCK_DIR" ] || [ -L "$INSTALL_LOCK_DIR" ] || return 0
    if [ -d "$INSTALL_LOCK_DIR" ] && [ ! -L "$INSTALL_LOCK_DIR" ] && \
        [ -f "$INSTALL_LOCK_DIR/pid" ] && [ ! -L "$INSTALL_LOCK_DIR/pid" ] && \
        [ -r "$INSTALL_LOCK_DIR/pid" ]; then
        IFS= read -r lifecycle_lock_owner <"$INSTALL_LOCK_DIR/pid" || true
        if valid_process_id "$lifecycle_lock_owner" && \
            [ "$lifecycle_lock_owner" = "${PPID:-}" ] && \
            kill -0 "$lifecycle_lock_owner" 2>/dev/null; then
            return 0
        fi
    fi
    printf '%s\n' 'io-workbench local runtime is being installed or removed; wait for that operation to finish before starting it.' >&2
    return 1
}

DEFAULT_PORT=8787
saved_port=
if [ -r "$RUNTIME_PORT_PATH" ]; then
    IFS= read -r saved_port <"$RUNTIME_PORT_PATH" || true
    if valid_port "$saved_port"; then
        DEFAULT_PORT=$saved_port
    fi
fi
PORT=${IO_WORKBENCH_PORT:-"$DEFAULT_PORT"}
valid_port "$PORT" || { printf '%s\n' 'io-workbench local runtime port is invalid.' >&2; exit 2; }

DEFAULT_WORKSPACE_ROOT="$TERMUX_HOME/projects"
saved_workspace_root=
if [ -r "$RUNTIME_WORKSPACE_ROOT_PATH" ]; then
    IFS= read -r saved_workspace_root <"$RUNTIME_WORKSPACE_ROOT_PATH" || true
    if [ -n "$saved_workspace_root" ]; then
        DEFAULT_WORKSPACE_ROOT=$saved_workspace_root
    fi
fi
WORKSPACE_ROOT=${IO_WORKBENCH_WORKSPACE_ROOT:-"$DEFAULT_WORKSPACE_ROOT"}
# The Android client is intentionally localhost-only. Do not inherit a host
# override that could accidentally expose its terminal and project APIs.
HOST=127.0.0.1
PID_FILE="$CONFIG_DIR/io-workbench.pid"
LOG_FILE="$CONFIG_DIR/logs/io-workbench.log"
BIN="$BIN_DIR/io-workbench"

export PATH="$BIN_DIR:$TERMUX_PREFIX/bin:$PATH"
export HOME="$TERMUX_HOME"
export IO_WORKBENCH_HOST="$HOST"
export IO_WORKBENCH_PORT="$PORT"
export IO_WORKBENCH_CONFIG_DIR="$CONFIG_DIR"
export IO_WORKBENCH_WORKSPACE_ROOT="$WORKSPACE_ROOT"
export IO_WORKBENCH_AUTH_REQUIRED=true
export IO_WORKBENCH_FCM_ENABLED=false
export IO_WORKBENCH_RAG_MODE=off
# Keep a random Termux-private bearer token on fresh local runtimes. This
# prevents an arbitrary Android app from claiming the unauthenticated
# first-user setup endpoint over shared loopback. Existing password-based
# runtimes have no token file and retain their current authentication flow.
unset IO_WORKBENCH_TOKEN
RUNTIME_TOKEN_AVAILABLE=0
runtime_token=
if [ -e "$RUNTIME_TOKEN_PATH" ] || [ -L "$RUNTIME_TOKEN_PATH" ]; then
    [ -f "$RUNTIME_TOKEN_PATH" ] && [ ! -L "$RUNTIME_TOKEN_PATH" ] && [ -r "$RUNTIME_TOKEN_PATH" ] || {
        printf '%s\n' "io-workbench local runtime token is not a readable regular file: $RUNTIME_TOKEN_PATH" >&2
        exit 1
    }
    IFS= read -r runtime_token <"$RUNTIME_TOKEN_PATH" || true
    valid_runtime_token "$runtime_token" || {
        printf '%s\n' "io-workbench local runtime token is invalid: $RUNTIME_TOKEN_PATH" >&2
        exit 1
    }
    export IO_WORKBENCH_TOKEN="$runtime_token"
    RUNTIME_TOKEN_AVAILABLE=1
fi
umask 077

mkdir -p "$CONFIG_DIR/logs" "$WORKSPACE_ROOT" || exit 1

process_start_time() {
    target_pid=$1
    [ -r "/proc/$target_pid/stat" ] || return 1
    process_stat=$(cat "/proc/$target_pid/stat" 2>/dev/null) || return 1
    # The command field is parenthesized and may itself contain spaces. Strip
    # through its final `) ` before counting fields; starttime is field 22 of
    # proc(5), hence field 20 of the remaining suffix.
    process_fields=${process_stat##*) }
    set -- $process_fields
    [ "$#" -ge 20 ] || return 1
    case "${20}" in
        ''|*[!0-9]*) return 1 ;;
    esac
    printf '%s\n' "${20}"
}

read_runtime_pid_record() {
    pid_record_pid=
    pid_record_start_time=
    pid_record_extra=
    [ -f "$PID_FILE" ] || return 1
    IFS=' ' read -r pid_record_pid pid_record_start_time pid_record_extra <"$PID_FILE" || return 1
    valid_process_id "$pid_record_pid" || return 1
    valid_process_id "$pid_record_start_time" || return 1
    [ -z "$pid_record_extra" ] || return 1
    return 0
}

runtime_process_matches() {
    candidate_pid=$1
    valid_process_id "$candidate_pid" || return 1
    [ -r "/proc/$candidate_pid/cmdline" ] || return 1
    [ -r "/proc/$candidate_pid/environ" ] || return 1

    # Do not treat an arbitrary process which merely happens to be called
    # io-workbench as this local runtime.  The executable, launch subcommand,
    # private configuration directory, loopback host, and saved port must all
    # agree.  `/proc` is readable only within the Termux UID here; no token or
    # other environment value is ever copied to stdout or a shell variable.
    command_path=$(tr '\0' '\n' <"/proc/$candidate_pid/cmdline" 2>/dev/null | sed -n '1p')
    command_subcommand=$(tr '\0' '\n' <"/proc/$candidate_pid/cmdline" 2>/dev/null | sed -n '2p')
    [ "$command_path" = "$BIN" ] || return 1
    [ "$command_subcommand" = start ] || return 1
    tr '\0' '\n' <"/proc/$candidate_pid/environ" 2>/dev/null | grep -Fqx "IO_WORKBENCH_CONFIG_DIR=$CONFIG_DIR" || return 1
    tr '\0' '\n' <"/proc/$candidate_pid/environ" 2>/dev/null | grep -Fqx "IO_WORKBENCH_HOST=$HOST" || return 1
    tr '\0' '\n' <"/proc/$candidate_pid/environ" 2>/dev/null | grep -Fqx "IO_WORKBENCH_PORT=$PORT" || return 1
    return 0
}

write_runtime_pid_record() {
    record_pid=$1
    record_start_time=$2
    valid_process_id "$record_pid" || return 1
    valid_process_id "$record_start_time" || return 1
    staged_pid_file="$PID_FILE.$$.new"
    printf '%s %s\n' "$record_pid" "$record_start_time" >"$staged_pid_file" || return 1
    chmod 600 "$staged_pid_file" || {
        rm -f "$staged_pid_file" 2>/dev/null || true
        return 1
    }
    mv -f "$staged_pid_file" "$PID_FILE" || {
        rm -f "$staged_pid_file" 2>/dev/null || true
        return 1
    }
}

discover_runtime_pid() {
    discovered_pid=
    discovered_start_time=
    for candidate_path in /proc/[0-9]*; do
        candidate_pid=${candidate_path##*/}
        runtime_process_matches "$candidate_pid" || continue
        candidate_start_time=$(process_start_time "$candidate_pid" || true)
        valid_process_id "$candidate_start_time" || continue
        # A loopback port permits at most one current listener, but never pick
        # one arbitrarily if a damaged environment somehow has two matching
        # process records.  The caller can then inspect the runtime safely.
        [ -z "$discovered_pid" ] || return 1
        discovered_pid=$candidate_pid
        discovered_start_time=$candidate_start_time
    done
    [ -n "$discovered_pid" ] || return 1
    runtime_pid=$discovered_pid
    runtime_start_time=$discovered_start_time
    return 0
}

running_pid() {
    runtime_pid=
    if read_runtime_pid_record && kill -0 "$pid_record_pid" 2>/dev/null; then
        observed_start_time=$(process_start_time "$pid_record_pid" || true)
        if [ "$observed_start_time" = "$pid_record_start_time" ] && runtime_process_matches "$pid_record_pid"; then
            runtime_pid=$pid_record_pid
            return 0
        fi
    fi

    # Android may restore the Termux foreground command service after a device
    # restart while its old wrapper PID record has gone away.  Rediscover only
    # this exact runtime and immediately repair the private record, so Status,
    # Doctor, Start, and Stop continue to manage the already-healthy host.
    discover_runtime_pid || return 1
    if ! write_runtime_pid_record "$runtime_pid" "$runtime_start_time"; then
        printf '%s\n' "io-workbench local runtime is running (PID $runtime_pid), but could not repair $PID_FILE" >&2
        return 1
    fi
    return 0
}

emit_connection_token() {
    [ "$RUNTIME_TOKEN_AVAILABLE" -eq 1 ] || {
        printf '%s\n' 'io-workbench local runtime has no paired token; use its existing password login instead.' >&2
        return 1
    }
    # Keep this as one exact, stdout-only record. The Android result callback
    # accepts it only for a successful explicit install or pairing request and
    # removes it before displaying command output or persisting diagnostics.
    printf '%s%s;port=%s\n' "$PAIRING_RECORD_PREFIX" "$runtime_token" "$PORT"
}

stop_runtime() {
    if ! running_pid; then
        rm -f "$PID_FILE"
        printf '%s\n' 'io-workbench is not running.'
        return 0
    fi

    stopping_pid=$runtime_pid
    if ! kill "$stopping_pid" 2>/dev/null && running_pid; then
        printf '%s\n' "could not stop io-workbench process $stopping_pid; inspect $LOG_FILE" >&2
        return 1
    fi
    stop_attempts=0
    while running_pid; do
        if [ "$stop_attempts" -ge 15 ]; then
            printf '%s\n' "io-workbench process $stopping_pid did not exit after 15 seconds; inspect $LOG_FILE" >&2
            return 1
        fi
        stop_attempts=$((stop_attempts + 1))
        sleep 1
    done
    rm -f "$PID_FILE"
    printf 'io-workbench stopped (PID %s)\n' "$stopping_pid"
}

start_runtime() {
    ensure_runtime_start_allowed || return 1
    if running_pid; then
        printf 'io-workbench is already running (PID %s) at http://%s:%s\n' "$runtime_pid" "$HOST" "$PORT"
        return 0
    fi
    rm -f "$PID_FILE"
    [ -x "$BIN" ] || { printf '%s\n' "io-workbench binary is missing: $BIN" >&2; return 1; }
    nohup "$BIN" start >>"$LOG_FILE" 2>&1 < /dev/null &
    started_pid=$!
    started_at=
    startup_attempts=0
    while [ "$startup_attempts" -lt 3 ]; do
        if ! kill -0 "$started_pid" 2>/dev/null; then
            printf '%s\n' "io-workbench exited during startup; inspect $LOG_FILE" >&2
            return 1
        fi
        started_at=$(process_start_time "$started_pid" || true)
        [ -n "$started_at" ] && break
        startup_attempts=$((startup_attempts + 1))
        sleep 1
    done
    if ! valid_process_id "$started_at"; then
        kill "$started_pid" 2>/dev/null || true
        printf '%s\n' "could not record the io-workbench process identity; inspect $LOG_FILE" >&2
        return 1
    fi
    printf '%s %s\n' "$started_pid" "$started_at" >"$PID_FILE" || {
        kill "$started_pid" 2>/dev/null || true
        printf '%s\n' "could not write $PID_FILE" >&2
        return 1
    }
    printf 'io-workbench started (PID %s) at http://%s:%s\n' "$started_pid" "$HOST" "$PORT"
}

case "${1:-start}" in
    start)
        start_runtime
        ;;
    run)
        shift
        ensure_runtime_start_allowed || exit 1
        # The Android installer first launches a detached copy long enough to
        # verify /health, then its one-shot callback asks Termux to own the
        # durable foreground host.  Replace only an already verified instance
        # for that private handoff; ordinary `run` stays non-disruptive when a
        # user has intentionally started the host from a Termux shell.
        replace_existing=0
        if [ "${1:-}" = --replace ]; then
            replace_existing=1
            shift
        fi
        if running_pid; then
            if [ "$replace_existing" -eq 1 ]; then
                stop_runtime || exit $?
            else
                printf 'io-workbench is already running (PID %s) at http://%s:%s\n' "$runtime_pid" "$HOST" "$PORT"
                exit 0
            fi
        fi
        rm -f "$PID_FILE"
        # `exec` below retains this shell's PID and `/proc` start time. Record
        # it before the replacement so status, stop, and doctor work for the
        # long-lived foreground RunCommand host just as they do for the
        # detached launcher mode.
        foreground_started_at=$(process_start_time "$$" || true)
        if ! valid_process_id "$foreground_started_at"; then
            printf '%s\n' "could not record the io-workbench foreground process identity; inspect $LOG_FILE" >&2
            exit 1
        fi
        printf '%s %s\n' "$$" "$foreground_started_at" >"$PID_FILE" || {
            printf '%s\n' "could not write $PID_FILE" >&2
            exit 1
        }
        exec "$BIN" start "$@"
        ;;
    stop)
        stop_runtime
        ;;
    restart)
        stop_runtime || exit $?
        start_runtime
        ;;
    status)
        if running_pid; then
            printf 'io-workbench process is running (PID %s) at http://%s:%s\n' "$runtime_pid" "$HOST" "$PORT"
        else
            rm -f "$PID_FILE"
            printf '%s\n' 'io-workbench process is not running.'
            exit 1
        fi
        ;;
    doctor)
        shift
        exec "$BIN" doctor "$@"
        ;;
    logs)
        exec tail -n "${2:-120}" "$LOG_FILE"
        ;;
    connection-token)
        [ "$#" -eq 1 ] || {
            printf '%s\n' 'Usage: io-workbench-local connection-token' >&2
            exit 2
        }
        emit_connection_token
        ;;
    *)
        printf '%s\n' 'Usage: io-workbench-local {start|run|stop|restart|status|doctor|logs|connection-token}' >&2
        exit 2
        ;;
esac
WRAPPER
if [ "$?" -ne 0 ]; then
    rm -f "$WRAPPER_STAGED" || true
    fail 'could not write the local runtime wrapper.'
fi
chmod 700 "$WRAPPER_STAGED" || {
    rm -f "$WRAPPER_STAGED" || true
    fail 'could not mark the local runtime wrapper executable.'
}
mv -f "$WRAPPER_STAGED" "$BIN_DIR/io-workbench-local" || {
    rm -f "$WRAPPER_STAGED" || true
    fail 'could not atomically install the local runtime wrapper.'
}

# Termux starts Bash as a login shell and its packaged /etc/profile sources a
# user's ~/.bashrc.  A fresh Termux install has no user startup files, so the
# wrapper above would otherwise be invisible from the normal interactive
# terminal even though the installer can call it by its absolute path.  Keep a
# small, idempotent block in the user-owned file rather than modifying a
# package-managed Termux profile.  The block uses $HOME at shell startup time,
# so it is safe to write without interpolating a caller-provided path.
ensure_user_bin_on_interactive_path() {
    BASHRC_PATH="$HOME/.bashrc"
    PATH_BLOCK_MARKER='# >>> io-workbench Termux user bin >>>'
    if [ -f "$BASHRC_PATH" ] && grep -Fq "$PATH_BLOCK_MARKER" "$BASHRC_PATH"; then
        return 0
    fi
    {
        printf '\n%s\n' "$PATH_BLOCK_MARKER"
        printf '%s\n' 'if [ -d "$HOME/.local/bin" ]; then'
        printf '%s\n' '    case ":${PATH-}:" in'
        printf '%s\n' '        *":$HOME/.local/bin:"*) ;;'
        printf '%s\n' '        *) export PATH="$HOME/.local/bin${PATH:+:$PATH}" ;;'
        printf '%s\n' '    esac'
        printf '%s\n' 'fi'
        printf '%s\n' '# <<< io-workbench Termux user bin <<<'
    } >>"$BASHRC_PATH" || fail 'could not add ~/.local/bin to the interactive Termux PATH.'
}
ensure_user_bin_on_interactive_path

install_cli() {
    label=$1
    package=$2
    note "installing $label through Termux npm."
    run_or_fail "$label installation" npm install --global "$package"
}

termux_npm_arch() {
    case "$(uname -m)" in
        aarch64|arm64) printf '%s\n' 'arm64' ;;
        x86_64|amd64) printf '%s\n' 'x64' ;;
        *) return 1 ;;
    esac
}

install_codex_cli() {
    npm_arch=$(termux_npm_arch) || fail "Codex CLI does not publish a supported Termux architecture for $(uname -m)."
    note 'installing Codex CLI with its Linux npm target.'
    run_or_fail 'Codex CLI installation' env npm_config_os=linux npm_config_cpu="$npm_arch" npm install --global --force '@openai/codex'
    cat >"$BIN_DIR/codex" <<'CODEX_WRAPPER' || fail 'could not write the Termux Codex wrapper.'
#!/data/data/com.termux/files/usr/bin/sh
set -u

TERMUX_PREFIX=${PREFIX:-/data/data/com.termux/files/usr}
CODEX_ENTRY="$TERMUX_PREFIX/lib/node_modules/@openai/codex/bin/codex.js"
NODE="$TERMUX_PREFIX/bin/node"
PROOT="$TERMUX_PREFIX/bin/proot"
CA_CERT="$TERMUX_PREFIX/etc/tls/cert.pem"
DNS_CONFIG="$TERMUX_PREFIX/etc/resolv.conf"

[ -x "$NODE" ] || { printf '%s\n' 'Codex needs the Termux nodejs-lts package.' >&2; exit 1; }
[ -r "$CODEX_ENTRY" ] || { printf '%s\n' 'Codex is not installed; rerun install-termux.sh --with-codex.' >&2; exit 1; }
[ -x "$PROOT" ] || { printf '%s\n' 'Codex needs the Termux proot package.' >&2; exit 1; }

# The Codex npm package contains a Linux musl binary. PRoot gives that child
# a conventional resolver path, while SSL_CERT_FILE keeps TLS on Termux's CA
# bundle. The Node launcher still resolves the package's selected architecture.
if [ -r "$CA_CERT" ] && [ -z "${SSL_CERT_FILE:-}" ]; then
    export SSL_CERT_FILE="$CA_CERT"
fi
if [ -r "$DNS_CONFIG" ]; then
    exec "$PROOT" --link2symlink -b "$DNS_CONFIG:/etc/resolv.conf" "$NODE" "$CODEX_ENTRY" "$@"
fi
exec "$PROOT" --link2symlink "$NODE" "$CODEX_ENTRY" "$@"
CODEX_WRAPPER
    chmod 700 "$BIN_DIR/codex" || fail 'could not mark the Termux Codex wrapper executable.'
    run_or_fail 'Codex CLI verification' "$BIN_DIR/codex" --version
}

install_claude_cli() {
    # 2.1.112 is a Node launcher that runs on Termux. Its bundled Linux
    # ripgrep cannot run against Android/Bionic, so route Claude through the
    # native Termux rg instead of allowing its vendor binary to be selected.
    install_cli 'Claude Code 2.1.112' '@anthropic-ai/claude-code@2.1.112'
    cat >"$BIN_DIR/claude" <<'CLAUDE_WRAPPER' || fail 'could not write the Termux Claude wrapper.'
#!/data/data/com.termux/files/usr/bin/sh
set -u

TERMUX_PREFIX=${PREFIX:-/data/data/com.termux/files/usr}
CLAUDE_ENTRY="$TERMUX_PREFIX/lib/node_modules/@anthropic-ai/claude-code/cli.js"
NODE="$TERMUX_PREFIX/bin/node"

[ -x "$NODE" ] || { printf '%s\n' 'Claude Code needs the Termux nodejs-lts package.' >&2; exit 1; }
[ -r "$CLAUDE_ENTRY" ] || { printf '%s\n' 'Claude Code is not installed; rerun install-termux.sh --with-claude.' >&2; exit 1; }

# Claude treats this explicit false setting as a request to resolve the system
# rg. That keeps file search in the Termux/Bionic environment instead of its
# bundled GNU/Linux binary.
export USE_BUILTIN_RIPGREP=false
exec "$NODE" "$CLAUDE_ENTRY" "$@"
CLAUDE_WRAPPER
    chmod 700 "$BIN_DIR/claude" || fail 'could not mark the Termux Claude wrapper executable.'
    run_or_fail 'Claude Code verification' "$BIN_DIR/claude" --version
}

install_gemini_cli() {
    # Gemini's npm shim has a /usr/bin/env node shebang, but Android has no
    # /usr/bin. Invoke its JavaScript entrypoint with the Termux Node binary
    # instead. Gemini's clipboard dependency also checks TERMUX_VERSION at
    # module-load time, so retain the normal Termux value and provide a safe
    # fallback for non-interactive server-launched shells.
    install_cli 'Gemini CLI' '@google/gemini-cli'
    cat >"$BIN_DIR/gemini" <<'GEMINI_WRAPPER' || fail 'could not write the Termux Gemini wrapper.'
#!/data/data/com.termux/files/usr/bin/sh
set -u

TERMUX_PREFIX=${PREFIX:-/data/data/com.termux/files/usr}
GEMINI_ENTRY="$TERMUX_PREFIX/lib/node_modules/@google/gemini-cli/bundle/gemini.js"
NODE="$TERMUX_PREFIX/bin/node"

[ -x "$NODE" ] || { printf '%s\n' 'Gemini CLI needs the Termux nodejs-lts package.' >&2; exit 1; }
[ -r "$GEMINI_ENTRY" ] || { printf '%s\n' 'Gemini CLI is not installed; rerun install-termux.sh --with-gemini.' >&2; exit 1; }

# The official CLI uses this to select its Android/Termux clipboard adapter.
# A foreground Termux shell already provides the real version; a server child
# may not inherit it even though it is still inside the same Termux runtime.
export TERMUX_VERSION=${TERMUX_VERSION:-0}
exec "$NODE" "$GEMINI_ENTRY" "$@"
GEMINI_WRAPPER
    chmod 700 "$BIN_DIR/gemini" || fail 'could not mark the Termux Gemini wrapper executable.'
    run_or_fail 'Gemini CLI verification' "$BIN_DIR/gemini" --version
}

[ "$INSTALL_CODEX" -eq 1 ] && install_codex_cli
[ "$INSTALL_CLAUDE" -eq 1 ] && install_claude_cli
[ "$INSTALL_GEMINI" -eq 1 ] && install_gemini_cli

if [ "$START_AFTER_INSTALL" -eq 1 ]; then
    record_install_progress starting 6
    note 'restarting the authenticated localhost host with the installed build.'
    if ! "$BIN_DIR/io-workbench-local" restart >>"$LOG_PATH" 2>&1; then
        fail "host startup failed; inspect $CONFIG_DIR/logs/io-workbench.log."
    fi
    attempts=0
    while [ "$attempts" -lt 20 ]; do
        if curl --fail --silent "http://$HOST:$PORT/health" >/dev/null 2>&1; then
            printf 'io-workbench Termux runtime is ready at http://%s:%s\n' "$HOST" "$PORT"
            if emit_runtime_pairing_record; then
                printf 'Open io-workbench Mobile and choose Connect local runtime.\n'
            else
                printf 'Open io-workbench Mobile and use the existing password-based local runtime login.\n'
            fi
            record_install_progress_success
            exit 0
        fi
        attempts=$((attempts + 1))
        sleep 1
    done
    fail "host did not pass its health check; inspect $CONFIG_DIR/logs/io-workbench.log."
fi

record_install_progress_success
printf '%s\n' 'io-workbench Termux runtime installed. Start it with: io-workbench-local start'
