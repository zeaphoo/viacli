#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="${SCRIPT_DIR}/dist"
DOCKERFILE_PATH="${SCRIPT_DIR}/Dockerfile"
BUILDER_NAME="viacli-builder"

mkdir -p "${OUT_DIR}"

if ! docker buildx version >/dev/null 2>&1; then
  echo "Error: docker buildx is not available" >&2
  exit 1
fi

if ! docker buildx inspect "${BUILDER_NAME}" >/dev/null 2>&1; then
  docker buildx create --name "${BUILDER_NAME}" --driver docker-container --use >/dev/null
else
  docker buildx use "${BUILDER_NAME}" >/dev/null
fi

docker buildx inspect --bootstrap >/dev/null

build_one() {
  local platform="$1"
  local arch_dir="$2"
  local dest_dir="${OUT_DIR}/${arch_dir}"

  mkdir -p "${dest_dir}"

  echo "========================================"
  echo "Building ${platform}..."
  echo "========================================"

  docker buildx build \
    --platform "${platform}" \
    --file "${DOCKERFILE_PATH}" \
    --output "type=local,dest=${dest_dir}" \
    "${SCRIPT_DIR}"

  chmod +x "${dest_dir}/viacli" || true
  echo "✓ Exported ${platform} -> ${dest_dir}/viacli"
  ls -lh "${dest_dir}/viacli" 2>/dev/null || true
}

# Build both architectures
build_one "linux/amd64" "amd64"
build_one "linux/arm64" "arm64"

echo ""
echo "========================================"
echo "✓ Build complete!"
echo "Output files:"
echo "  - ${OUT_DIR}/amd64/viacli"
echo "  - ${OUT_DIR}/arm64/viacli"
echo "========================================"
