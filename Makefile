# Local CI via Dagger — no GitHub Actions, no external runners.
# Run `make ci` before pushing to verify everything passes locally.

.PHONY: ci lint test build docker clean

# Run the full pipeline: lint, test, and build
ci:
	dagger call all --source=. --progress=plain

# Individual steps
lint:
	dagger call lint --source=. --progress=plain

test:
	dagger call test --source=. --progress=plain

build:
	dagger call build --source=. --progress=plain

# Build the container image locally
docker:
	dagger call docker --source=. -o zerochaind-image.tar

# Remove build artifacts
clean:
	rm -f zerochaind-image.tar
	cargo clean
