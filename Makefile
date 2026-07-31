.PHONY: build test clean install benchmark fmt clippy check

# Build the project
build:
	cargo build --release

# Run tests
test:
	cargo test --all

# Run tests with coverage
test-coverage:
	cargo test --all -- --nocapture

# Clean build artifacts
clean:
	cargo clean

# Install locally
install:
	bash scripts/install.sh

# Run benchmarks
benchmark:
	cargo bench

# Format code
fmt:
	cargo fmt --all -- --check

# Run clippy
clippy:
	cargo clippy --all-targets -- -D warnings

# Check everything
check: fmt clippy test

# Build for different targets
build-linux:
	cross build --target x86_64-unknown-linux-musl --release

build-macos:
	cross build --target x86_64-apple-darwin --release

build-windows:
	cross build --target x86_64-pc-windows-gnu --release

# Development setup
dev-setup:
	rustup target add x86_64-unknown-linux-musl
	rustup target add x86_64-apple-darwin
	rustup target add x86_64-pc-windows-gnu
	cargo install cross
