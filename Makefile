.PHONY: build release test bundle clean

# Debug build.
build:
	cargo build

# Optimized build.
release:
	cargo build --release

test:
	cargo test

# macOS: wrap the release binary in a proper .app bundle so Finder launches it
# as a GUI app (no stray terminal window). Output: dist/mavlog.app.
bundle: release
	packaging/macos/bundle.sh target/release/mavlog dist

clean:
	cargo clean
	rm -rf dist
