#!/usr/bin/env bash
set -eo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/env_fig13.sh"
if [[ -z "${SSH_AUTH_SOCK:-}" ]]; then eval "$(ssh-agent -s)"; fi
ssh-add "${FIRESIM_SSH_KEY}"
cd "${FIRESIM_HOME}"
source sourceme-manager.sh --skip-ssh-setup
firesim kill
