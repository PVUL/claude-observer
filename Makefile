# claude-observer — build/install helpers.
# `make install` is the default: builds release + puts `claude-observer` on your PATH.
.PHONY: install build uninstall

# Compile (release) and install the binary to ~/.cargo/bin (on PATH by default).
# Re-run to upgrade after pulling.
install:
	cargo install --path .

# Dev build only; binary at ./target/release/claude-observer (not installed).
build:
	cargo build --release

# Remove the installed binary.
uninstall:
	cargo uninstall claude-observer
