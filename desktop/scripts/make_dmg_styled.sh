#!/usr/bin/env bash
# 把已构建好的 .app 打成样式化 DMG：空白窗口 + 固定图标布局，无背景图。
#
# 用法：
#   bash desktop/scripts/make_dmg_styled.sh <app 路径> <输出 dmg 路径>
# 例：
#   bash desktop/scripts/make_dmg_styled.sh \
#     "desktop/src-tauri/target/release/bundle/macos/QM Unlock.app" \
#     "dist/QM Unlock_1.0.0_aarch64.dmg"
#
# 三条铁律（违反就会「能打开但样式全错」或直接失败）：
#   1. 布局一律来自 dmg-template.DS_Store，绝不用 AppleScript / osascript 指挥 Finder。
#      沙箱或无 GUI 环境下 AppleScript 会失败、超时或静默不生效。
#   2. 不做背景图。空白窗口跟随系统外观，浅色系统下黑标签、深色系统下白标签都可读；
#      也彻底避开「背景白底 / 背景被拉伸错位 / alias 书签解析失败」这一整类问题。
#   3. 挂载一律用自定义挂载点 -mountpoint /tmp/xxx，绝不用默认的 /Volumes/<名字>。
#
# 为什么不用 dmgbuild / create-dmg：
#   dmgbuild 生成的 backgroundImageAlias 是残缺的（324 字节，可用的是 600+），
#   且不写新版 Finder 优先使用的 pBBk 书签；create-dmg 依赖 AppleScript 指挥 Finder，
#   违反铁律 1。现在既然不要背景图，两者更没有存在的必要。
#
# 模板从哪来：
#   desktop/scripts/dmg-template.DS_Store 由 dmg_layout.py 从一份 Finder 认可的样例
#   .DS_Store 派生：只替换条目名、把图标尺寸调到 84、删掉背景引用，窗口 bounds 等
#   字节原样保留。改布局请改 dmg_layout.py 重新生成，不要手改模板。
#
# 卷名为什么是 "<app> Setup"：
#   访达会按卷名恢复「上次的窗口大小」，一旦恢复就忽略 .DS_Store 里的 WindowBounds，
#   窗口尺寸就不受控。用一个从未打开过的新卷名可以避开。
#
# 流程（7 步）：
#   ① 暂存 stage：app + Applications 符号链接 + 安装说明 + .VolumeIcon.icns + .DS_Store
#   ② 清 xattr 并 ad-hoc 签名、逐个校验嵌套代码
#   ③ makehybrid -hfs 生成混合映像（不探测 /Volumes，沙箱友好）→ convert 成可读写 UDRW
#   ④ 挂载到 /tmp 自定义挂载点，设卷图标属性、清 FinderInfo、去组写权限、清 .fseventsd
#   ⑤ rm 旧产物 → convert 成只读压缩 UDZO（zlib-level=9）
#   ⑥ 清理临时文件
#   ⑦ 只读挂载回读校验（条目齐全、无背景图、签名有效、布局参数）
set -euo pipefail

APP_PATH="${1:?用法: make_dmg_styled.sh <app 路径> <输出 dmg 路径>}"
OUT_DMG="${2:?用法: make_dmg_styled.sh <app 路径> <输出 dmg 路径>}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

[[ -d "$APP_PATH" ]] || { echo "找不到应用：$APP_PATH" >&2; exit 1; }

APP_NAME="$(basename "$APP_PATH" .app)"
VOL_NAME="$APP_NAME Setup"
APP_ITEM="$APP_NAME.app"
VOL_ICON="$REPO_ROOT/desktop/src-tauri/icons/icon.icns"
NOTES_SRC="$SCRIPT_DIR/DMG_INSTALL_ZH-CN.txt"
NOTES_NAME="① 安装与故障排查.txt"
TEMPLATE="$SCRIPT_DIR/dmg-template.DS_Store"

[[ -f "$TEMPLATE" ]] || { echo "缺少布局模板：${TEMPLATE}（用 dmg_layout.py 生成）" >&2; exit 1; }

STAGE="/tmp/qm-dmg-stage-$$"
MNT="/tmp/qm-dmg-mnt-$$"
VMNT="/tmp/qm-dmg-verify-$$"
HYBRID="/tmp/qm-hybrid-$$.dmg"
RW_DMG="/tmp/qm-rw-$$.dmg"
DEV=""

cleanup() {
  if [[ -n "$DEV" ]]; then hdiutil detach "$DEV" -force >/dev/null 2>&1 || true; fi
  for m in "$MNT" "$VMNT"; do
    if [[ -d "$m" ]]; then hdiutil detach "$m" -force >/dev/null 2>&1 || true; fi
    rm -rf "$m"
  done
  rm -rf "$STAGE"
  rm -f "$HYBRID" "$RW_DMG"
}
trap cleanup EXIT

echo "① 暂存内容…"
rm -rf "$STAGE"; mkdir -p "$STAGE"
ditto "$APP_PATH" "$STAGE/$APP_ITEM"
ln -s /Applications "$STAGE/Applications"
if [[ -f "$NOTES_SRC" ]]; then cp "$NOTES_SRC" "$STAGE/$NOTES_NAME"; fi
if [[ -f "$VOL_ICON" ]]; then cp "$VOL_ICON" "$STAGE/.VolumeIcon.icns"; fi
cp "$TEMPLATE" "$STAGE/.DS_Store"                     # 布局，铁律 1
echo "   条目：$(ls -A "$STAGE" | tr '\n' ' ')"

echo "② 清理扩展属性并 ad-hoc 签名…"
# 先清 xattr：Tauri 产物的签名本身是不完整的（会报 code has no resources but
# signature indicates they must be present），必须重签；而 codesign --strict 又不允许
# 任何附带数据，所以清理要在签名之前。
xattr -cr "$STAGE/$APP_ITEM" 2>/dev/null || true
codesign --force --deep --sign - --timestamp=none "$STAGE/$APP_ITEM" 2>&1 | sed 's/^/   /'
codesign --verify --deep --strict "$STAGE/$APP_ITEM" && echo "   签名有效"

# 逐个校验嵌套的 framework / helper / dylib / 可执行文件。
# --deep 只保证主可执行文件，嵌套代码要单独验，否则打出来的包在用户机器上可能被拦。
echo "   校验嵌套代码…"
NESTED_COUNT=0
for nested_root in \
  "$STAGE/$APP_ITEM/Contents/MacOS" \
  "$STAGE/$APP_ITEM/Contents/Frameworks" \
  "$STAGE/$APP_ITEM/Contents/Helpers" \
  "$STAGE/$APP_ITEM/Contents/PlugIns" \
  "$STAGE/$APP_ITEM/Contents/XPCServices"; do
  [[ -d "$nested_root" ]] || continue
  while IFS= read -r -d '' nested_code; do
    if [[ -f "$nested_code" ]] && ! file -b "$nested_code" | grep -q 'Mach-O'; then
      continue
    fi
    codesign --verify --deep --strict "$nested_code"
    NESTED_COUNT=$((NESTED_COUNT + 1))
  done < <(
    # 两个匹配分支必须整体括起来再 -print0，否则 -print0 只作用于文件分支，
    # .app / .framework 目录不会进入逐项校验循环。
    find "$nested_root" \
      \( \
        \( -type d \( -name '*.app' -o -name '*.framework' \) \) -o \
        \( -type f \( -name '*.dylib' -o -perm -111 \) \) \
      \) -print0
  )
done
echo "   嵌套代码校验通过：${NESTED_COUNT} 项"

echo "③ 混合映像 → 可读写映像…"
rm -f "$HYBRID" "$RW_DMG"
hdiutil makehybrid -hfs -hfs-volume-name "$VOL_NAME" -o "$HYBRID" "$STAGE" >/dev/null
hdiutil convert -format UDRW -o "$RW_DMG" "$HYBRID" >/dev/null
echo "   已生成可读写映像"

echo "④ 挂载设卷图标属性…"
rm -rf "$MNT"; mkdir -p "$MNT"
# -nobrowse -noautoopen：不在 Finder 侧栏显示、不自动开窗
DEV="$(hdiutil attach -readwrite -noverify -noautoopen -nobrowse -mountpoint "$MNT" "$RW_DMG" | tail -1 | awk '{print $1}')"
[[ -n "$DEV" ]] || { echo "挂载失败" >&2; exit 1; }
echo "   挂载点：${MNT}（${DEV}）"
if command -v SetFile >/dev/null 2>&1; then
  SetFile -c icnC "$MNT/.VolumeIcon.icns" 2>/dev/null || true   # creator type
  SetFile -a C "$MNT" 2>/dev/null || true                       # custom-icon 属性
  echo "   卷图标属性已设置"
else
  echo "   ⚠️  没有 SetFile，卷图标会显示为默认磁盘图标（xcode-select --install 可补）"
fi
chmod -Rf go-w "$MNT" &>/dev/null || true                        # 去组/其他写权限
# makehybrid -hfs 会给 .icns 和可执行文件写入 HFS 文件类型/creator，挂载后表现为
# com.apple.FinderInfo 扩展属性（实测 20 个）。codesign --strict 一律拒绝这类附带数据
# （"resource fork, Finder information, or similar detritus not allowed"），必须在卷内清掉。
# 注意只清 app：卷根和 .VolumeIcon.icns 的 FinderInfo 正是自定义卷图标所依赖的，不能动。
xattr -cr "$MNT/$APP_ITEM" 2>/dev/null || true
if codesign --verify --deep --strict "$MNT/$APP_ITEM" 2>/dev/null; then
  echo "   卷内 app 签名校验通过（FinderInfo 已清）"
else
  echo "❌ 卷内 app 签名校验失败，不继续打包：" >&2
  codesign --verify --deep --strict -vvv "$MNT/$APP_ITEM" 2>&1 | head -5 >&2
  exit 1
fi
rm -rf "$MNT/.fseventsd" 2>/dev/null || true                     # 清挂载产生的事件目录
hdiutil detach "$DEV" >/dev/null
DEV=""

echo "⑤ 转只读压缩 UDZO…"
mkdir -p "$(dirname "$OUT_DMG")"
rm -f "$OUT_DMG"                                                 # 不删旧产物 convert 会报错
hdiutil convert "$RW_DMG" -format UDZO -imagekey zlib-level=9 -o "$OUT_DMG" >/dev/null
hdiutil verify "$OUT_DMG" >/dev/null && echo "   映像完整性校验通过"

echo "⑥ 清理临时文件…"
rm -rf "$STAGE"; rm -f "$HYBRID" "$RW_DMG"

echo "⑦ 只读回读校验…"
FAIL=0
rm -rf "$VMNT"; mkdir -p "$VMNT"
hdiutil attach -readonly -nobrowse -noautoopen -noverify -mountpoint "$VMNT" "$OUT_DMG" >/dev/null

for item in "$APP_ITEM" "Applications" ".VolumeIcon.icns" ".DS_Store"; do
  if [[ -e "$VMNT/$item" || -L "$VMNT/$item" ]]; then echo "   ✓ $item"; else echo "   ✗ 缺 $item" >&2; FAIL=1; fi
done
if [[ -e "$VMNT/.background.png" || -d "$VMNT/.background" ]]; then
  echo "   ✗ 卷里不该有背景图（要求空白窗口）" >&2; FAIL=1
else
  echo "   ✓ 无背景图（空白窗口）"
fi
if [[ -f "$VMNT/$NOTES_NAME" ]]; then echo "   ✓ $NOTES_NAME"; else echo "   ✗ 缺 $NOTES_NAME" >&2; FAIL=1; fi
if [[ -L "$VMNT/Applications" ]]; then echo "   ✓ Applications 是符号链接"; else echo "   ✗ Applications 不是符号链接" >&2; FAIL=1; fi

BIN_MTIME="$(stat -f '%Sm' "$VMNT/$APP_ITEM/Contents/MacOS/"* 2>/dev/null | head -1)"
echo "   · app 内二进制构建时间：$BIN_MTIME"
if codesign --verify --deep --strict "$VMNT/$APP_ITEM" 2>/dev/null; then
  echo "   ✓ 卷内 app 签名有效"
else
  echo "   ✗ 卷内 app 签名校验失败" >&2; FAIL=1
fi

if command -v python3 >/dev/null 2>&1 && python3 -c "import ds_store" >/dev/null 2>&1; then
  if ! python3 - "$VMNT/.DS_Store" <<'PY'
import sys
from ds_store import DSStore

bad = 0
with DSStore.open(sys.argv[1], "r") as d:
    for e in d:
        code = e.code.decode() if isinstance(e.code, bytes) else e.code
        if code == "Iloc":
            print("   · Iloc %-26s (%s, %s)" % (e.filename, e.value[0], e.value[1]))
        elif code == "bwsp":
            print("   · WindowBounds =", e.value.get("WindowBounds"))
        elif code == "icvp":
            bt = e.value.get("backgroundType")
            print("   · icvp backgroundType=%s iconSize=%s 含背景alias=%s"
                  % (bt, e.value.get("iconSize"), "backgroundImageAlias" in e.value))
            if bt == 2 or "backgroundImageAlias" in e.value:
                print("   ✗ 仍带背景引用，不是空白窗口"); bad = 1
sys.exit(bad)
PY
  then
    FAIL=1
  fi
else
  echo "   · （未安装 ds_store，跳过 .DS_Store 回读）"
fi

hdiutil detach "$VMNT" >/dev/null
rm -rf "$VMNT"

echo
if [[ "$FAIL" -ne 0 ]]; then
  echo "❌ 校验有失败项，产物已生成但不可用：$OUT_DMG" >&2
  exit 1
fi
echo "✅ 完成：$OUT_DMG"
ls -lh "$OUT_DMG" | awk '{print "   大小:", $5}'
echo "   双击打开确认：空白窗口 660×400、app 在左、Applications 在右、说明文件在右上角。"
