#!/usr/bin/env bash
# 从源码安装本仓库的全部 bin（每个 bin 是一个独立 package）。
# 用法：
#   ./scripts/install.sh                 # 安装全部
#   ./scripts/install.sh weather weixin  # 只安装指定能力
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
cd "${ROOT_DIR}"

# 能力名 -> crate 目录（crate 名即 cargo install --path 的目录名）
ALL=(weather finance weixin qqbot chrome_fetch web_search hazards markets space rss system)
LEGACY=(viacli)

if [[ $# -gt 0 ]]; then
  TARGETS=("$@")
else
  TARGETS=("${ALL[@]}" "${LEGACY[@]}")
fi

for name in "${TARGETS[@]}"; do
  dir="crates/${name}"
  if [[ ! -f "${dir}/Cargo.toml" ]]; then
    echo "未知目标：${name}（可用：${ALL[*]} ${LEGACY[*]}）" >&2
    exit 1
  fi
  echo "==> cargo install --path ${dir}"
  cargo install --path "${dir}" --locked
done

echo ""
echo "已安装的 bin："
for name in "${TARGETS[@]}"; do
  grep -m1 '^name' "crates/${name}/Cargo.toml" >/dev/null || true
done
ls -1 "${CARGO_HOME:-$HOME/.cargo}/bin" | grep -E '^viacli' || true
