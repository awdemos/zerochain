# zerochain

## Deployment

This project has a Dagger module (`dagger.json` in `./`).
From the repository root, run:

```bash
# list available functions
dagger call --help -m ./
```

Common deployment functions:

```bash
# Build compiles the workspace in release mode and returns the
dagger call -m ./ build
# BuildAll compiles all binaries in the workspace and returns the
dagger call -m ./ build-all
# BuildEnv returns a container with the Rust nightly toolchain,
dagger call -m ./ build-env
# Publish builds the zerochaind image and pushes it to a
dagger call -m ./ publish
```

You may need to export required tokens before calling deploy functions (e.g., `GH_TOKEN`, `CLOUDFLARE_API_TOKEN`, `REGISTRY_TOKEN`).
