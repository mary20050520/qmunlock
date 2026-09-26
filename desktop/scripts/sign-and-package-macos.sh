#!/usr/bin/env bash
# CI 用的 macOS 打包入口：解析 target / arch / 版本号 / 产物路径，
# 然后把实际打包委托给 make_dmg_styled.sh（本地手动打包用的是同一个脚本）。
#
# 用法： ./scripts/sign-and-package-macos.sh <rust-target>
# 例：   ./scripts/sign-and-package-macos.sh aarch64-apple-darwin
#
# 之所以只保留一层薄封装：以前这里自己用 hdiutil create -srcfolder 打了一个
# 无背景、无图标布局的普通 DMG，和本地脚本产出的样式化 DMG 不一致。
# 现在两条路径合一，签名、嵌套代码校验、卷图标、背景与布局、回读校验全部只有一份实现。
set -euo pipefail

target="${1:?usage: sign-and-package-macos.sh <rust-target>}"

case "$target" in
  aarch64-apple-darwin) arch="aarch64" ;;
  x86_64-apple-darwin) arch="x64" ;;
  *)
    echo "Unsupported macOS target: $target" >&2
    exit 2
    ;;
esac

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

bundle_dir="src-tauri/target/$target/release/bundle"
app_path="$(find "$bundle_dir/macos" -maxdepth 1 -type d -name '*.app' -print -quit)"
if [[ -z "$app_path" ]]; then
  echo "macOS app bundle was not generated for $target" >&2
  exit 1
fi

app_name="$(basename "$app_path" .app)"
version="$(node -p 'require("./package.json").version')"
dmg_dir="$bundle_dir/dmg"
dmg_path="$dmg_dir/${app_name}_${version}_${arch}.dmg"

mkdir -p "$dmg_dir"
echo "Packaging $app_path"
echo "         -> $dmg_path"

# 打包引擎：ad-hoc 签名、嵌套代码校验、makehybrid、卷图标、清 FinderInfo、
# 转 UDZO、只读回读校验（含 .DS_Store 布局参数），全在 make_dmg_styled.sh 里。
"$script_dir/make_dmg_styled.sh" "$app_path" "$dmg_path"

echo "Created and verified $dmg_path"
