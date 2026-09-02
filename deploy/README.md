# Production deployment prerequisites

The tag workflow in `.github/workflows/release.yml` deploys the published Linux
release archive after GitHub Release publication. Its production job is pinned
to a repo-scoped self-hosted runner carrying the `io-workbench-deploy` label.
Put that runner on the private deployment host; this avoids exposing SSH to the
public internet while the job retains its explicit SSH, checksum, rollback,
and health-check boundary. Do not attach this label to a general-purpose runner
or use it from untrusted workflows.

Use `io-workbench-deploy-runner.service.example` only after registering a
dedicated runner with the repository and giving it the `io-workbench-deploy`
label. The runner service should run as the deployment account, while the
separate sudoers policy grants only the release install and rollback commands.

1. Create a dedicated service user, durable data directory, workspace root, and
   `/opt/io-workbench` binary directory on the deployment host.
2. Install `io-workbench.service.example` as a systemd unit after replacing the
   example paths and user. Enable it once with `systemctl enable --now
   io-workbench`.
3. Provision the deployment SSH user with narrowly scoped passwordless
   permission for the exact operations used by the workflow: `install -d` for
   the live-binary directory, `install` for the replacement binary, `cp` for a
   timestamped rollback copy, `systemctl restart` for the configured service,
   and `tee` for the adjacent `revision.json`. Scope each command to the
   configured paths and service rather than granting unrestricted sudo. Record
   the server's expected OpenSSH known-hosts line during this controlled setup;
   do not fetch and trust a host key dynamically during a deployment. Do not
   point `DEPLOY_LIVE_BINARY` at a developer checkout's `target/release`
   directory. Start from `io-workbench-release.sudoers.example`, replace its
   account/path/service values, validate it with `visudo -cf`, then install it
   at `/etc/sudoers.d/io-workbench-release` with mode `0440`.
4. Create the Cloudflare DNS/tunnel route for `workbench.giofahreza.com`, then
   merge `cloudflared-workbench-ingress.example.yml` before the catch-all
   ingress rule and restart cloudflared. The tunnel must forward WebSockets as
   well as HTTP.
5. Store the deployment settings as GitHub Actions secrets:

   - `DEPLOY_HOST`
   - `DEPLOY_USER`
   - `DEPLOY_SSH_KEY`
   - `DEPLOY_SSH_KNOWN_HOSTS`
   - `DEPLOY_REMOTE_STAGE_DIR`
   - `DEPLOY_LIVE_BINARY`
   - `DEPLOY_SERVICE_NAME`
   - `DEPLOY_HEALTH_URL`

   `DEPLOY_SSH_PORT` and `DEPLOY_PUBLIC_HEALTH_URL` are optional. The latter
   defaults to `https://workbench.giofahreza.com/health`.

   Set `DEPLOY_SSH_KNOWN_HOSTS` to the complete known-hosts entry (use
   `[host]:port` notation for a non-default port). The deployment
   workflow restores the timestamped prior binary if the replacement fails its
   local service or health check.

The tag workflow also needs these Android signing secrets to publish updatable
APK releases: `IOWB_ANDROID_KEYSTORE_BASE64`,
`IOWB_ANDROID_KEYSTORE_PASSWORD`, `IOWB_ANDROID_KEY_ALIAS`, and
`IOWB_ANDROID_KEY_PASSWORD`. If `apps` remains a private submodule, provide
`IOWB_SUBMODULES_TOKEN` with read-only Contents access.
