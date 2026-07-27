# io-workbench Desktop

This folder contains the first desktop packaging layer around the Rust server
binary and embedded UI.

`io-workbench-desktop.sh` starts the release binary, waits for `/health`, opens
the default browser, and shuts the child server down when the launcher exits.

Build the binary first:

```sh
cargo build --release -p iowb-cli --bin io-workbench
```

Run the desktop launcher:

```sh
apps/desktop/io-workbench-desktop.sh
```

Package metadata is in `desktop-package.json` so native packagers can wrap the
same command without changing server behavior.

