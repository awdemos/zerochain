package main

import (
	"context"
	"fmt"

	"dagger/zerochain/internal/dagger"
)

const (
	rustImage = "rust:latest"
)

type Zerochain struct{}

// BuildEnv returns a container with the Rust nightly toolchain, project source,
// and cargo cache volumes mounted.
func (m *Zerochain) BuildEnv(
	// +defaultPath="/"
	source *dagger.Directory,
) *dagger.Container {
	registryCache := dag.CacheVolume("zerochain-cargo-registry")
	gitCache := dag.CacheVolume("zerochain-cargo-git")
	targetCache := dag.CacheVolume("zerochain-cargo-target")

	return dag.Container().
		From(rustImage).
		WithExec([]string{"rustup", "default", "nightly"}).
		WithDirectory("/src", source).
		WithWorkdir("/src").
		WithMountedCache("/usr/local/cargo/registry", registryCache).
		WithMountedCache("/usr/local/cargo/git", gitCache).
		WithMountedCache("/src/target", targetCache).
		WithExec([]string{"cargo", "--version"})
}

// Build compiles the workspace in release mode and returns the binary.
func (m *Zerochain) Build(
	ctx context.Context,
	// +defaultPath="/"
	source *dagger.Directory,
) *dagger.File {
	return m.BuildEnv(source).
		WithExec([]string{"cargo", "build", "--release", "--workspace"}).
		File("/src/target/release/zerochain")
}

// BuildAll compiles all binaries in the workspace and returns the build directory.
func (m *Zerochain) BuildAll(
	ctx context.Context,
	// +defaultPath="/"
	source *dagger.Directory,
) *dagger.Directory {
	return m.BuildEnv(source).
		WithExec([]string{"cargo", "build", "--release", "--workspace"}).
		Directory("/src/target/release")
}

// Test runs all workspace tests and returns the output.
func (m *Zerochain) Test(
	ctx context.Context,
	// +defaultPath="/"
	source *dagger.Directory,
) (string, error) {
	return m.BuildEnv(source).
		WithExec([]string{"cargo", "test", "--workspace"}).
		Stdout(ctx)
}

// Lint runs clippy and rustfmt checks, returning any warnings or errors.
func (m *Zerochain) Lint(
	ctx context.Context,
	// +defaultPath="/"
	source *dagger.Directory,
) (string, error) {
	clippy, err := m.BuildEnv(source).
		WithExec([]string{"cargo", "clippy", "--workspace", "--", "-D", "warnings"}).
		Stdout(ctx)
	if err != nil {
		return "", fmt.Errorf("clippy failed: %w\n%s", err, clippy)
	}

	fmtCheck, err := m.BuildEnv(source).
		WithExec([]string{"cargo", "fmt", "--check", "--all"}).
		Stdout(ctx)
	if err != nil {
		return "", fmt.Errorf("rustfmt check failed: %w\n%s", err, fmtCheck)
	}

	return "clippy and rustfmt checks passed", nil
}

// Docker builds the zerochaind container image from the existing Dockerfile.
func (m *Zerochain) Docker(
	ctx context.Context,
	// +defaultPath="/"
	source *dagger.Directory,
) *dagger.Container {
	return source.DockerBuild()
}

// All runs the full CI pipeline: lint, test, and build.
func (m *Zerochain) All(
	ctx context.Context,
	// +defaultPath="/"
	source *dagger.Directory,
) (string, error) {
	lintResult, err := m.Lint(ctx, source)
	if err != nil {
		return "", fmt.Errorf("lint failed: %w", err)
	}

	testResult, err := m.Test(ctx, source)
	if err != nil {
		return "", fmt.Errorf("tests failed: %w", err)
	}

	m.Build(ctx, source)

	return fmt.Sprintf("CI passed.\n  lint: %s\n  tests: %s\n  build: ok", lintResult, truncate(testResult, 200)), nil
}

func truncate(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}
