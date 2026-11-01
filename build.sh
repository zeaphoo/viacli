#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="${SCRIPT_DIR}/dist"
DOCKERFILE_PATH="${SCRIPT_DIR}/Dockerfile"
BUILDER_NAME="viacli-builder"

# 需要导出的 bin（与 Dockerfile 的 VIACLI_BINS 保持一致）
BINS=(viacli viacli-weather viacli-finance viacli-weixin viacli-qqbot viacli-fetch viacli-search
      viacli-hazards viacli-markets viacli-space viacli-rss viacli-sys)

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

  for b in "${BINS[@]}"; do
    chmod +x "${dest_dir}/${b}"
    echo "✓ ${platform} -> ${dest_dir}/${b}"
    ls -lh "${dest_dir}/${b}" | awk '{print "   " $5 "  " $9}'
  done

  # 打成一个 tar 包，方便整组拷贝到目标机器
  tar -czf "${OUT_DIR}/viacli-${arch_dir}.tar.gz" -C "${dest_dir}" "${BINS[@]}"
  echo "✓ ${OUT_DIR}/viacli-${arch_dir}.tar.gz"
}

# Build both architectures
build_one "linux/amd64" "amd64"
build_one "linux/arm64" "arm64"

echo ""
echo "========================================"
echo "✓ Build complete!"
echo "Output:"
for arch in amd64 arm64; do
  for b in "${BINS[@]}"; do
    echo "  - ${OUT_DIR}/${arch}/${b}"
  done
  echo "  - ${OUT_DIR}/viacli-${arch}.tar.gz"
done
echo "========================================"
