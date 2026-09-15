#!/data/data/com.termux/files/usr/bin/sh
# Remove the io-workbench localhost runtime installed by install-termux.sh.
#
# This deliberately does not remove Termux itself, Termux packages, provider
# CLIs or their credentials/configuration.  The Android client invokes this
# script through Termux RUN_COMMAND, so stdout is reserved for one small,
# token-free result record; all diagnostics belong on stderr.

set -u

RESULT_PREFIX='__IOWB_TERMUX_UNINSTALL_V1__='
RESULT_RUN_ID='00000000-0000-0000-0000-000000000000'
RESULT_STATE=failed
RESULT_RUNTIME=unchanged
RESULT_PROJECTS=retained
RESULT_REASON=invalid_arguments
RESULT_EMITTED=0
RUN_ID_SET=0

TERMUX_HOME=${HOME:-/data/data/com.termux/files/home}
RUNTIME_HOME="$TERMUX_HOME/.local/share/io-workbench"
BIN_DIR="$TERMUX_HOME/.local/bin"
CONFIG_DIR="$TERMUX_HOME/.io-workbench"
INSTALL_LOCK_DIR="$RUNTIME_HOME/.termux-install.lock"
WRAPPER="$BIN_DIR/io-workbench-local"
RUNTIME_BINARY="$BIN_DIR/io-workbench"
PID_FILE="$CONFIG_DIR/io-workbench.pid"
WORKSPACE_SETTING="$CONFIG_DIR/termux-runtime-workspace-root"
# This deliberately lives outside the runtime/configuration paths that an
# uninstall removes.  It contains only the same fixed, credential-free result
# record returned to Android, so a later app launch can recover if Android
# drops the one-shot RUN_COMMAND callback.
UNINSTALL_STATUS_DIR="$TERMUX_HOME/.cache/io-workbench"
UNINSTALL_STATUS_FILE="$UNINSTALL_STATUS_DIR/termux-uninstall-result"

USER_ACTION=
RUNTIME_PRESENT=0
PROJECT_DELETE_PLAN=
PROJECT_WORKSPACE_ROOT=
PROJECT_RESULT=retained
PROJECT_RESULT_REASON=none
UNINSTALL_LOCK_HELD=0
UNINSTALL_LOCK_CREATED_RUNTIME_HOME=0
UNINSTALL_STATUS_READY=0

result_record() {
    # Keep this exact shape in sync with the strict Android-side parser.  It
    # intentionally contains neither command output nor any local bearer.
    printf '%sversion=1;run_id=%s;state=%s;runtime=%s;projects=%s;reason=%s\n' \
        "$RESULT_PREFIX" "$RESULT_RUN_ID" "$RESULT_STATE" "$RESULT_RUNTIME" \
        "$RESULT_PROJECTS" "$RESULT_REASON"
}

emit_result() {
    [ "$RESULT_EMITTED" -eq 0 ] || return 0
    RESULT_EMITTED=1
    result_record
}

finish() {
    RESULT_STATE=$1
    RESULT_RUNTIME=$2
    RESULT_PROJECTS=$3
    RESULT_REASON=$4
    exit "${5:-1}"
}

note() {
    printf '%s\n' "io-workbench Termux uninstall: $*" >&2
}

path_exists() {
    [ -e "$1" ] || [ -L "$1" ]
}

valid_run_id() {
    candidate=$1
    # Mobile uses java.util.UUID.  A canonical UUID keeps this callback
    # correlation field bounded and prevents an arbitrary shell argument from
    # changing the fixed stdout protocol.
    case "$candidate" in
        ????????-????-????-????-????????????) ;;
        *) return 1 ;;
    esac
    compact_candidate=$(printf '%s' "$candidate" | tr -d '-')
    [ "${#compact_candidate}" -eq 32 ] || return 1
    case "$compact_candidate" in
        *[!0123456789abcdefABCDEF]*) return 1 ;;
    esac
    return 0
}

valid_process_id() {
    candidate=$1
    case "$candidate" in
        ''|*[!0-9]*) return 1 ;;
    esac
    [ "$candidate" -ge 1 ] 2>/dev/null
}

require_termux() {
    case "${PREFIX:-}" in
        */com.termux/files/usr) ;;
        *)
            note 'this uninstaller must run from the official Termux app.'
            return 1
            ;;
    esac
    case "$TERMUX_HOME" in
        /*) ;;
        *)
            note 'Termux HOME is not an absolute path; refusing to remove anything.'
            return 1
            ;;
    esac
    [ "$TERMUX_HOME" != / ] || {
        note 'Termux HOME resolved to /; refusing to remove anything.'
        return 1
    }
    [ -d "$TERMUX_HOME" ] && [ ! -L "$TERMUX_HOME" ] || {
        note 'Termux HOME is not a regular non-symlink directory; refusing to remove anything.'
        return 1
    }
    return 0
}

prepare_status_storage() {
    # Do not follow a user-controlled cache symlink just to improve callback
    # recovery.  Failure here is non-fatal: the normal PendingIntent result
    # still reports the uninstall outcome.
    if path_exists "$TERMUX_HOME/.cache" && { [ ! -d "$TERMUX_HOME/.cache" ] || [ -L "$TERMUX_HOME/.cache" ]; }; then
        return 1
    fi
    if ! mkdir -p "$UNINSTALL_STATUS_DIR"; then
        return 1
    fi
    if [ ! -d "$UNINSTALL_STATUS_DIR" ] || [ -L "$UNINSTALL_STATUS_DIR" ]; then
        return 1
    fi
    if ! chmod 700 "$UNINSTALL_STATUS_DIR"; then
        return 1
    fi
    UNINSTALL_STATUS_READY=1
    return 0
}

persist_result() {
    [ "$UNINSTALL_STATUS_READY" -eq 1 ] || return 0
    status_temp="$UNINSTALL_STATUS_FILE.$$.new"
    (umask 077; result_record >"$status_temp") || return 0
    chmod 600 "$status_temp" 2>/dev/null || {
        rm -f "$status_temp" 2>/dev/null || true
        return 0
    }
    mv -f "$status_temp" "$UNINSTALL_STATUS_FILE" || {
        rm -f "$status_temp" 2>/dev/null || true
        return 0
    }
    return 0
}

clear_previous_result() {
    # A completion record belongs to exactly one Android request.  Remove a
    # previous regular file before this new request begins, so a lost callback
    # can never be mistaken for an earlier uninstall.  Do not follow or remove
    # anything other than the exact, ordinary status file.
    [ "$UNINSTALL_STATUS_READY" -eq 1 ] || return 0
    path_exists "$UNINSTALL_STATUS_FILE" || return 0
    [ -f "$UNINSTALL_STATUS_FILE" ] && [ ! -L "$UNINSTALL_STATUS_FILE" ] || return 1
    rm -f "$UNINSTALL_STATUS_FILE"
}

parse_arguments() {
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --run-id)
                [ "$#" -ge 2 ] || return 1
                [ "$RUN_ID_SET" -eq 0 ] || return 1
                valid_run_id "$2" || return 1
                RESULT_RUN_ID=$2
                RUN_ID_SET=1
                shift 2
                ;;
            --keep-projects)
                [ -z "$USER_ACTION" ] || return 1
                USER_ACTION=keep
                shift
                ;;
            --delete-projects)
                [ -z "$USER_ACTION" ] || return 1
                USER_ACTION=delete
                shift
                ;;
            *)
                return 1
                ;;
        esac
    done
    [ -n "$USER_ACTION" ] && [ "$RUN_ID_SET" -eq 1 ]
}

runtime_paths_are_safe() {
    # Do not traverse a symlinked .local hierarchy or delete a non-directory
    # configuration/runtime target.  The installer owns only the leaves below;
    # its siblings (including provider wrappers) remain untouched.
    for parent in "$TERMUX_HOME/.local" "$TERMUX_HOME/.local/bin" "$TERMUX_HOME/.local/share"; do
        if path_exists "$parent" && { [ ! -d "$parent" ] || [ -L "$parent" ]; }; then
            note "expected parent is not a regular non-symlink directory: $parent"
            return 1
        fi
    done
    for directory in "$RUNTIME_HOME" "$CONFIG_DIR"; do
        if path_exists "$directory" && { [ ! -d "$directory" ] || [ -L "$directory" ]; }; then
            note "runtime path is not a regular non-symlink directory: $directory"
            return 1
        fi
    done
    for file in "$WRAPPER" "$RUNTIME_BINARY"; do
        if path_exists "$file" && { [ ! -f "$file" ] || [ -L "$file" ]; }; then
            note "runtime path is not a regular non-symlink file: $file"
            return 1
        fi
    done
    return 0
}

# Reclaim only the exact lock shape an interrupted installer/uninstaller
# leaves behind: a non-symlink directory containing one ordinary `pid` file
# whose owner is no longer alive.  This runs only after the user explicitly
# retries removal, never from a background status probe.  Any extra entry,
# link, unreadable file, or live PID remains a busy lock.
reclaim_dead_empty_install_lock() {
    path_exists "$INSTALL_LOCK_DIR" || return 0
    [ -d "$INSTALL_LOCK_DIR" ] && [ ! -L "$INSTALL_LOCK_DIR" ] || return 1
    [ -f "$INSTALL_LOCK_DIR/pid" ] && [ ! -L "$INSTALL_LOCK_DIR/pid" ] && \
        [ -r "$INSTALL_LOCK_DIR/pid" ] || return 1
    for lock_entry in "$INSTALL_LOCK_DIR"/* "$INSTALL_LOCK_DIR"/.[!.]* "$INSTALL_LOCK_DIR"/..?*; do
        path_exists "$lock_entry" || continue
        [ "$lock_entry" = "$INSTALL_LOCK_DIR/pid" ] || return 1
    done
    IFS= read -r lock_pid <"$INSTALL_LOCK_DIR/pid" || true
    if valid_process_id "$lock_pid" && kill -0 "$lock_pid" 2>/dev/null; then
        return 1
    fi
    note 'reclaiming an empty local-runtime lock whose owner is no longer running.'
    rm -f "$INSTALL_LOCK_DIR/pid" && rmdir "$INSTALL_LOCK_DIR"
}

detect_runtime() {
    if path_exists "$RUNTIME_HOME" || path_exists "$CONFIG_DIR" || \
        path_exists "$WRAPPER" || path_exists "$RUNTIME_BINARY"; then
        RUNTIME_PRESENT=1
    else
        RUNTIME_PRESENT=0
    fi
}

verified_runtime_process_matches() {
    candidate_pid=$1
    case "$candidate_pid" in
        ''|*[!0-9]*) return 1 ;;
    esac
    [ -r "/proc/$candidate_pid/cmdline" ] && [ -r "/proc/$candidate_pid/environ" ] || return 1
    candidate_binary=$(tr '\0' '\n' <"/proc/$candidate_pid/cmdline" 2>/dev/null | sed -n '1p')
    candidate_subcommand=$(tr '\0' '\n' <"/proc/$candidate_pid/cmdline" 2>/dev/null | sed -n '2p')
    [ "$candidate_binary" = "$RUNTIME_BINARY" ] || return 1
    [ "$candidate_subcommand" = start ] || return 1
    tr '\0' '\n' <"/proc/$candidate_pid/environ" 2>/dev/null | \
        grep -Fqx "IO_WORKBENCH_CONFIG_DIR=$CONFIG_DIR" || return 1
    tr '\0' '\n' <"/proc/$candidate_pid/environ" 2>/dev/null | \
        grep -Fqx 'IO_WORKBENCH_HOST=127.0.0.1' || return 1
    return 0
}

verified_runtime_pids() {
    for candidate_path in /proc/[0-9]*; do
        candidate_pid=${candidate_path##*/}
        verified_runtime_process_matches "$candidate_pid" || continue
        printf '%s\n' "$candidate_pid"
    done
}

stop_verified_runtime_processes() {
    remaining_pids=$(verified_runtime_pids)
    [ -z "$remaining_pids" ] && return 0
    note 'a verified io-workbench host remains; stopping that exact host before removal.'
    for remaining_pid in $remaining_pids; do
        kill "$remaining_pid" 2>/dev/null || return 1
    done
    stop_attempts=0
    while :; do
        remaining_pids=$(verified_runtime_pids)
        [ -z "$remaining_pids" ] && return 0
        if [ "$stop_attempts" -ge 15 ]; then
            note 'a verified io-workbench host did not exit after 15 seconds; leaving the runtime intact.'
            return 1
        fi
        stop_attempts=$((stop_attempts + 1))
        sleep 1
    done
}

stop_runtime_safely() {
    [ "$RUNTIME_PRESENT" -eq 1 ] || return 0

    # Do not execute the runtime wrapper during uninstall.  A partial or
    # tampered wrapper can create its configured workspace/log directories
    # before it reaches `stop`, which is surprising when the user explicitly
    # chose Delete projects.  Instead, inspect only a process whose exact
    # binary path, `start` subcommand, and loopback/configuration environment
    # prove that it belongs to this runtime.  This safely handles a
    # missing/non-executable wrapper too, without ever using broad matching or
    # pkill.  If no such process exists, removal can proceed.
    stop_verified_runtime_processes
}

workspace_root_is_safe() {
    candidate=$1
    [ -n "$candidate" ] || return 1
    [ "${#candidate}" -le 4096 ] || return 1
    case "$candidate" in
        "$TERMUX_HOME"/*) ;;
        *) return 1 ;;
    esac
    case "$candidate" in
        "$TERMUX_HOME/.local"|"$TERMUX_HOME/.local/"*|\
        "$TERMUX_HOME/.io-workbench"|"$TERMUX_HOME/.io-workbench/"*|\
        "$TERMUX_HOME/.termux"|"$TERMUX_HOME/.termux/"*|\
        "$TERMUX_HOME/.ssh"|"$TERMUX_HOME/.ssh/"*|\
        "$TERMUX_HOME/.config"|"$TERMUX_HOME/.config/"*|\
        "$TERMUX_HOME/.cache"|"$TERMUX_HOME/.cache/"*|\
        "$TERMUX_HOME/storage"|"$TERMUX_HOME/storage/"*)
            return 1
            ;;
    esac

    # Validate every existing ancestor rather than relying only on a lexical
    # prefix.  That prevents a configured path beneath a symlinked directory
    # from being considered a Termux-home project directory.
    relative=${candidate#"$TERMUX_HOME"/}
    current=$TERMUX_HOME
    remaining=$relative
    while [ -n "$remaining" ]; do
        component=${remaining%%/*}
        if [ "$component" = "$remaining" ]; then
            remaining=
        else
            remaining=${remaining#*/}
        fi
        case "$component" in
            ''|.|..) return 1 ;;
        esac
        current=$current/$component
        if [ -n "$remaining" ]; then
            [ -d "$current" ] && [ ! -L "$current" ] || return 1
        elif path_exists "$current"; then
            [ -d "$current" ] && [ ! -L "$current" ] || return 1
        fi
    done
    return 0
}

prepare_project_deletion() {
    PROJECT_DELETE_PLAN=unsafe
    PROJECT_WORKSPACE_ROOT=
    # Only a runtime-owned, private setting is evidence that a directory is
    # this local runtime's project root.  Falling back to ~/projects after a
    # partial/old uninstall could erase an unrelated Termux folder.
    if ! path_exists "$WORKSPACE_SETTING"; then
        note 'the runtime-owned workspace setting is absent; keeping projects.'
        return 1
    fi
    [ -f "$WORKSPACE_SETTING" ] && [ ! -L "$WORKSPACE_SETTING" ] && [ -r "$WORKSPACE_SETTING" ] || {
        note 'the saved workspace root is not a readable regular non-symlink file; keeping projects.'
        return 1
    }
    IFS= read -r PROJECT_WORKSPACE_ROOT <"$WORKSPACE_SETTING" || true
    if ! workspace_root_is_safe "$PROJECT_WORKSPACE_ROOT"; then
        note 'the saved workspace root is unsafe for recursive deletion; keeping projects.'
        return 1
    fi
    if path_exists "$PROJECT_WORKSPACE_ROOT"; then
        # workspace_root_is_safe already verified it is a real directory.
        PROJECT_DELETE_PLAN=delete
    else
        PROJECT_DELETE_PLAN=absent
    fi
    return 0
}

remove_runtime_non_source_paths() {
    # All paths were preflighted as ordinary files/directories under the
    # Termux home hierarchy.  Do not remove .local itself: it can contain
    # user-installed provider wrappers and unrelated tools.
    if path_exists "$WRAPPER" && ! rm -f "$WRAPPER"; then
        note "could not remove $WRAPPER"
        return 1
    fi
    if path_exists "$RUNTIME_BINARY" && ! rm -f "$RUNTIME_BINARY"; then
        note "could not remove $RUNTIME_BINARY"
        return 1
    fi
    if path_exists "$CONFIG_DIR" && ! rm -rf "$CONFIG_DIR"; then
        note "could not remove $CONFIG_DIR"
        return 1
    fi
    if path_exists "$WRAPPER" || path_exists "$RUNTIME_BINARY" || \
        path_exists "$CONFIG_DIR"; then
        note 'one or more io-workbench runtime paths remain after removal.'
        return 1
    fi
    return 0
}

remove_runtime_source_path() {
    # Keep the shared installer lock until every requested project operation
    # has finished.  An installer that starts after this deletion sees a fully
    # removed runtime rather than racing with project deletion.
    if path_exists "$RUNTIME_HOME" && ! rm -rf "$RUNTIME_HOME"; then
        note "could not remove $RUNTIME_HOME"
        return 1
    fi
    if path_exists "$RUNTIME_HOME"; then
        note 'one or more io-workbench runtime paths remain after removal.'
        return 1
    fi
    return 0
}

delete_projects() {
    [ "$PROJECT_DELETE_PLAN" = delete ] || return 1
    # Recheck the exact directory immediately before removal.  A changed or
    # symlinked root becomes a safe partial uninstall instead of a recursive
    # deletion outside the Termux project area.
    workspace_root_is_safe "$PROJECT_WORKSPACE_ROOT" || return 1
    path_exists "$PROJECT_WORKSPACE_ROOT" || return 0
    rm -rf "$PROJECT_WORKSPACE_ROOT" || return 1
    ! path_exists "$PROJECT_WORKSPACE_ROOT"
}

release_uninstall_lock() {
    if [ "$UNINSTALL_LOCK_HELD" -eq 1 ]; then
        rm -f "$INSTALL_LOCK_DIR/pid" 2>/dev/null || true
        rmdir "$INSTALL_LOCK_DIR" 2>/dev/null || true
    fi
    if [ "$UNINSTALL_LOCK_CREATED_RUNTIME_HOME" -eq 1 ]; then
        # If the uninstall did not get as far as removing the runtime, leave
        # no empty source directory merely because we reserved the install
        # lock.  It is removed only when still empty.
        rmdir "$RUNTIME_HOME" 2>/dev/null || true
    fi
    UNINSTALL_LOCK_HELD=0
    UNINSTALL_LOCK_CREATED_RUNTIME_HOME=0
    return 0
}

acquire_uninstall_lock() {
    [ "$RUNTIME_PRESENT" -eq 1 ] || return 0
    if ! path_exists "$RUNTIME_HOME"; then
        if ! mkdir "$RUNTIME_HOME" 2>/dev/null; then
            note 'could not reserve the local runtime directory for uninstall.'
            return 2
        fi
        UNINSTALL_LOCK_CREATED_RUNTIME_HOME=1
    fi
    if ! mkdir "$INSTALL_LOCK_DIR" 2>/dev/null; then
        if path_exists "$INSTALL_LOCK_DIR"; then
            note 'an io-workbench Termux installation lock exists; wait for installation to finish before uninstalling.'
            return 1
        fi
        note 'could not reserve the local runtime install lock for uninstall.'
        return 2
    fi
    if ! chmod 700 "$INSTALL_LOCK_DIR" || ! printf '%s\n' "$$" >"$INSTALL_LOCK_DIR/pid"; then
        rm -f "$INSTALL_LOCK_DIR/pid" 2>/dev/null || true
        rmdir "$INSTALL_LOCK_DIR" 2>/dev/null || true
        note 'could not record the local runtime uninstall lock owner.'
        return 2
    fi
    UNINSTALL_LOCK_HELD=1
    return 0
}

on_signal() {
    note 'uninstall interrupted; keeping Android-side state unchanged until a later successful retry.'
    RESULT_STATE=failed
    RESULT_RUNTIME=unchanged
    RESULT_PROJECTS=retained
    RESULT_REASON=remove_failed
    exit 1
}

trap 'release_uninstall_lock; persist_result; emit_result' EXIT
trap 'on_signal' HUP INT TERM

if ! parse_arguments "$@"; then
    note 'usage: uninstall-termux.sh --run-id <uuid> (--keep-projects|--delete-projects)'
    finish failed unchanged retained invalid_arguments 2
fi

if ! require_termux; then
    finish failed unchanged retained invalid_arguments 2
fi

if prepare_status_storage; then
    clear_previous_result || note 'could not clear the previous non-sensitive uninstall recovery record.'
else
    note 'could not prepare the non-sensitive uninstall recovery record.'
fi

detect_runtime
if ! runtime_paths_are_safe; then
    finish failed unchanged retained remove_failed 1
fi

# A normal live installer owns its lock and remains busy.  A prior process
# that died before its EXIT trap may leave only a dead, empty pid lock; an
# explicit user retry can reclaim precisely that harmless shape so Android is
# never stuck in an uninstalling state forever.
if path_exists "$INSTALL_LOCK_DIR" && ! reclaim_dead_empty_install_lock; then
    note 'an io-workbench Termux installation lock exists; wait for installation to finish before uninstalling.'
    finish failed unchanged retained busy 1
fi

# Reserve the installer's own lock while removal is in progress.  A new
# installer will see this live PID and fail rather than recreating files while
# they are being removed. A retry may have reclaimed only an empty dead-pid
# lock above; any live or nonstandard lock remains rejected.
acquire_uninstall_lock
lock_status=$?
if [ "$lock_status" -ne 0 ]; then
    if [ "$lock_status" -eq 1 ]; then
        finish failed unchanged retained busy 1
    fi
    finish failed unchanged retained remove_failed 1
fi

if ! stop_runtime_safely; then
    finish failed unchanged retained stop_failed 1
fi

if [ "$USER_ACTION" = delete ]; then
    # Do this before removing CONFIG_DIR, which stores the configured root.
    # An unsafe root intentionally does not prevent removal of the host, but
    # it does turn the final result into a clear partial uninstall.
    prepare_project_deletion || true
fi

if [ "$RUNTIME_PRESENT" -eq 1 ] && ! remove_runtime_non_source_paths; then
    finish failed unchanged retained remove_failed 1
fi

if [ "$USER_ACTION" = keep ]; then
    PROJECT_RESULT=kept
else
    case "$PROJECT_DELETE_PLAN" in
        absent)
            PROJECT_RESULT=absent
            ;;
        delete)
            if delete_projects; then
                PROJECT_RESULT=deleted
            else
                PROJECT_RESULT=retained
                PROJECT_RESULT_REASON=project_remove_failed
                note 'could not remove the configured projects directory; retaining it while completing runtime removal.'
            fi
            ;;
        *)
            PROJECT_RESULT=retained
            PROJECT_RESULT_REASON=unsafe_workspace
            note 'the configured projects root is unsafe for recursive deletion; retaining it while completing runtime removal.'
            ;;
    esac
fi

if [ "$RUNTIME_PRESENT" -eq 1 ]; then
    if ! remove_runtime_source_path; then
        finish failed unchanged "$PROJECT_RESULT" remove_failed 1
    fi
    RESULT_RUNTIME=removed
else
    RESULT_RUNTIME=absent
fi

if [ "$PROJECT_RESULT_REASON" = none ]; then
    finish succeeded "$RESULT_RUNTIME" "$PROJECT_RESULT" none 0
fi
finish partial "$RESULT_RUNTIME" "$PROJECT_RESULT" "$PROJECT_RESULT_REASON" 0
