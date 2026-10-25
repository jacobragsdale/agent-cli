#!/usr/bin/env bash
# Install or update agent-cli from the team's Artifact Keeper, on Ubuntu (WSL
# included), 22.04 or newer, x86_64:
#
#   curl -fsSL https://keeper.contoso.example/api/v1/repositories/agent-cli/download/latest/install.sh | bash
#
# Run it again to update; every step is idempotent. It installs
#   ~/.local/bin/agent-cli                  the binary, checked against its .sha256
#   ~/.config/agent-cli/team.toml           the team's config, replaced on every run
#   ~/.config/agent-cli/config.toml         yours: on the first run only, created
#                                           (0600) or given [core] include
#   ~/.local/opt/oracle/instantclient_23_26 Oracle Instant Client Basic Light,
#                                           pinned and checked, with its run path
#                                           set and libaio.so.1 linked inside it
# with sudo only for the apt packages that are missing (unzip, patchelf,
# libaio), then runs `agent-cli config pull` and `agent-cli doctor`.
#
#   AGENT_CLI_RELEASES        where releases are (release-build.sh sets the
#                             default from team.toml's [core] releases)
#   AGENT_CLI_VERSION         a version to install, such as 0.2.0; default latest
#   AGENT_CLI_ORACLE_ZIP_URL  the same Instant Client zip from a mirror (its
#                             sha256 must still match)
set -euo pipefail

RELEASES_DEFAULT=https://keeper.contoso.example/api/v1/repositories/agent-cli/download
ORACLE_ZIP_URL=https://download.oracle.com/otn_software/linux/instantclient/2326400/instantclient-basiclite-linux.x64-23.26.4.0.0.zip
ORACLE_ZIP_SHA256=d049e593bba0041feb15c14ea894e2f1a437377462b99d9bdb6b7ab30b2dd660
# The directory the zip unpacks to; team.toml's [sql] oracle_client_dir names it.
ORACLE_DIR=instantclient_23_26
TARBALL=agent-cli-x86_64-linux.tar.gz

say() { echo "install: $*"; }
die() { echo "install: $*" >&2; exit 1; }
fetch() { curl -fsSL --retry 3 --connect-timeout 15 "$@"; }

# Installs the apt packages that are missing, with sudo unless already root.
packages() {
    installed() { dpkg-query -W -f='${Status}' "$1" 2>/dev/null | grep -q 'ok installed'; }
    local missing=() pkg aio=present
    for pkg in unzip patchelf; do
        installed "$pkg" || missing+=("$pkg")
    done
    installed libaio1t64 || installed libaio1 || aio=missing
    [ ${#missing[@]} -gt 0 ] || [ "$aio" = missing ] || return 0

    local run=()
    if [ "$(id -u)" -ne 0 ]; then
        command -v sudo >/dev/null ||
            die "there is no sudo to install these with apt: ${missing[*]} libaio1t64 (libaio1 before 24.04)"
        run=(sudo)
    fi
    "${run[@]}" env DEBIAN_FRONTEND=noninteractive apt-get update -qq
    if [ "$aio" = missing ]; then
        # 24.04 renamed libaio1 to libaio1t64 (the 64-bit time_t transition).
        if apt-cache show libaio1t64 >/dev/null 2>&1; then missing+=(libaio1t64); else missing+=(libaio1); fi
    fi
    say "installing with apt: ${missing[*]}"
    "${run[@]}" env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "${missing[@]}" >/dev/null
}

# Oracle Instant Client, unpacked once per pinned zip. The zip's libraries
# find each other only through LD_LIBRARY_PATH, so their run path is set to
# their own directory, and 24.04's libaio.so.1t64 is linked as the
# libaio.so.1 they ask for, inside that directory, so nothing system-wide
# changes.
oracle() {
    local root=$HOME/.local/opt/oracle
    local dir=$root/$ORACLE_DIR
    if [ "$(cat "$dir/.agent-cli-zip-sha256" 2>/dev/null)" != "$ORACLE_ZIP_SHA256" ]; then
        say "downloading Oracle Instant Client Basic Light"
        fetch -o "$tmp/oracle.zip" "${AGENT_CLI_ORACLE_ZIP_URL:-$ORACLE_ZIP_URL}"
        echo "$ORACLE_ZIP_SHA256  $tmp/oracle.zip" | sha256sum -c --quiet - ||
            die "the Instant Client zip's sha256 is not $ORACLE_ZIP_SHA256; is AGENT_CLI_ORACLE_ZIP_URL the same file?"
        unzip -q "$tmp/oracle.zip" -d "$tmp/oracle"
        local lib
        for lib in "$tmp/oracle/$ORACLE_DIR"/{libclntsh.so.*,libclntshcore.so.*,libnnz.so}; do
            # shellcheck disable=SC2016 # $ORIGIN is the loader's, not the shell's
            [ -L "$lib" ] || patchelf --set-rpath '$ORIGIN' "$lib"
        done
        echo "$ORACLE_ZIP_SHA256" >"$tmp/oracle/$ORACLE_DIR/.agent-cli-zip-sha256"
        mkdir -p "$root"
        rm -rf "$dir"
        mv "$tmp/oracle/$ORACLE_DIR" "$dir"
    fi
    local system=/usr/lib/x86_64-linux-gnu
    if [ ! -e "$system/libaio.so.1" ] && [ -e "$system/libaio.so.1t64" ]; then
        ln -sfn "$system/libaio.so.1t64" "$dir/libaio.so.1"
    fi
    say "Oracle Instant Client in $dir"
}

# Adds the include to the person's own config.toml, on the first run only:
# after that the file is theirs, and a missing include is only reported.
include() {
    local first=$1 config=$2 team=$3
    local line="include = [\"${team/#"$HOME"\//\~/}\"]"
    if [ ! -e "$config" ]; then
        (umask 077 && cat >"$config" <<EOF
# Your own agent-cli settings. They win over the team's, which install.sh
# replaces on every run in
#   $team
# A key here replaces the team's, and an entry with the same name
# ([[sql.connection]] name = "...") changes only the keys it sets. The
# team's logins come from Key Vault; a credential set here replaces the
# team's for that entry. \`agent-cli config\` shows what is read.
[core]
$line
EOF
        )
        say "wrote $config (0600), including the team's config"
    elif grep -Eq '^[[:space:]]*include[[:space:]]*=' "$config"; then
        :
    elif [ "$first" = 1 ] && ! grep -Eq '^[[:space:]]*\[core\]' "$config"; then
        printf '\n[core]\n%s\n' "$line" >>"$config"
        say "added [core] $line to $config"
    else
        say "$config does not include the team's config; add this line under [core] in it:"
        say "  $line"
    fi
}

main() {
    [ "$(uname -s)-$(uname -m)" = Linux-x86_64 ] || die "agent-cli is built for Linux on x86_64 (WSL Ubuntu), not $(uname -s) $(uname -m)"
    command -v dpkg-query >/dev/null || die "install.sh is for Ubuntu (WSL included); dpkg-query is missing"
    local base=${AGENT_CLI_RELEASES:-$RELEASES_DEFAULT}
    base=${base%/}
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT

    local version=${AGENT_CLI_VERSION:-latest}
    version=${version#v}
    if [ "$version" = latest ]; then
        version=$(fetch "$base/latest/VERSION") || die "cannot read $base/latest/VERSION; is AGENT_CLI_RELEASES right, and the network up?"
    fi
    say "agent-cli $version from $base"

    packages

    # The binary: checked, then moved into place in one step, so a running
    # agent-cli is never half-written.
    (cd "$tmp" && fetch -O "$base/$version/$TARBALL.sha256") || die "there is no agent-cli $version at $base"
    (cd "$tmp" && fetch -O "$base/$version/$TARBALL" && sha256sum -c --quiet "$TARBALL.sha256") ||
        die "the download of $base/$version/$TARBALL failed or does not match its .sha256"
    tar -xzf "$tmp/$TARBALL" -C "$tmp" agent-cli
    local bin_dir=$HOME/.local/bin
    mkdir -p "$bin_dir"
    install -m 0755 "$tmp/agent-cli" "$bin_dir/.agent-cli.new"
    mv -f "$bin_dir/.agent-cli.new" "$bin_dir/agent-cli"
    local bin=$bin_dir/agent-cli
    say "installed $bin"

    local config_dir=${XDG_CONFIG_HOME:-$HOME/.config}/agent-cli
    local config=${AGENT_CLI_CONFIG:-$config_dir/config.toml}
    local team=$config_dir/team.toml first=0
    mkdir -p "$config_dir" "$(dirname "$config")"
    [ -e "$team" ] || first=1
    fetch -o "$tmp/team.toml" "$base/$version/team.toml" || die "cannot download $base/$version/team.toml"
    install -m 0600 "$tmp/team.toml" "$team"
    say "wrote $team"
    include "$first" "$config" "$team"

    oracle

    case ":$PATH:" in
        *":$bin_dir:"*) ;;
        *) if grep -qs '\.local/bin' "$HOME/.profile"; then
               say "$bin_dir is not on your PATH yet: open a new terminal (and restart Cursor or VS Code), which adds it"
           else
               say "$bin_dir is not on your PATH: add this line to ~/.profile, then open a new terminal:"
               say "  export PATH=\"\$HOME/.local/bin:\$PATH\""
           fi ;;
    esac

    if command -v az >/dev/null && az account show >/dev/null 2>&1; then
        say "reading the team's shared logins from Key Vault (agent-cli config pull)"
        "$bin" config pull || say "agent-cli config pull failed (above); run it again once that is fixed"
    else
        say "the team's shared logins come from Key Vault with your Azure sign-in: run \`az login\`, then \`agent-cli config pull\`"
    fi

    say "checking every service (agent-cli doctor); a failed row's hint is the next step"
    "$bin" doctor || true

    say "done. Put this line in your agents' instructions (a global AGENTS.md, Cursor rules or Copilot instructions):"
    echo
    echo "agent-cli is installed: it reaches the team's work services; run \`agent-cli\` to see what is configured, then \`agent-cli search WORDS\`."
}

main "$@"
