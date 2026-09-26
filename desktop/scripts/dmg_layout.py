#!/usr/bin/env python3
"""从样例 DMG 的 .DS_Store 派生本项目的 DMG 布局模板（无背景图版）。

为什么需要这个脚本
==================
DMG 的窗口尺寸、图标坐标、图标尺寸全部编码在卷根目录的 .DS_Store 里。
可靠做法是复用一份「已被 Finder 认可过」的 .DS_Store，而不是用 AppleScript 去指挥
Finder 现场设置（沙箱/无 GUI 下会失败或静默不生效），也不是用 dmgbuild 生成。

样例的 .DS_Store 里写死了它自己的条目名，直接复制会导致 app 图标坐标不生效，
所以要把条目名替换掉。窗口 bounds、图标尺寸等字节原样保留。

不做背景图
==========
最终决定是空白窗口：icvp 里删掉 backgroundImageAlias、不写 pBBk 书签、
backgroundType 置 0。这样既没有「背景白底/错位」一类问题，也不依赖任何
alias/书签解析，模板与卷名彻底解耦。

用法
====
  python3 dmg_layout.py <样例 .DS_Store> <输出 .DS_Store> <app 条目名> <说明文件条目名>

生成结果会自校验：重新解析输出文件，逐条打印确认。
"""

from __future__ import annotations

import sys

from ds_store import DSStore, DSStoreEntry

# 图标坐标（逻辑像素，窗口内容区 660x400）。
# app 与 Applications 居中一排；说明文件放右上角；隐藏文件推到窗口可视区外。
POS_APP = (180, 190)
POS_APPS = (480, 190)
POS_NOTES = (592, 70)
POS_HIDDEN = {
    ".DS_Store": (100, 600),
    ".VolumeIcon.icns": (200, 600),
}


def main(argv: list[str]) -> int:
    if len(argv) != 5:
        print("用法: dmg_layout.py <样例.DS_Store> <输出.DS_Store> <app条目名> <说明文件条目名>")
        return 2
    src, dst, app_item, notes_item = argv[1:5]

    with DSStore.open(src, "r") as d:
        entries = {}
        for e in d:
            code = e.code.decode() if isinstance(e.code, bytes) else e.code
            # typecode 必须沿用源条目的，写错 Finder 就不认
            entries[(e.filename, code)] = (e.type, e.value)

    bwsp_type, bwsp = entries[(".", "bwsp")]
    icvp_type, icvp_raw = entries[(".", "icvp")]
    vsrn_type, vsrn = entries[(".", "vSrn")]
    iloc_type = entries[("Applications", "Iloc")][0]

    icvp = dict(icvp_raw)
    # 空白窗口：去掉背景图引用，backgroundType 置 0（跟随系统外观的默认底）
    icvp.pop("backgroundImageAlias", None)
    icvp["backgroundType"] = 0
    # 图标尺寸：样例是 96，偏大；84 更克制
    icvp["iconSize"] = 84.0

    iloc = {
        app_item: POS_APP,
        "Applications": POS_APPS,
        notes_item: POS_NOTES,
        **POS_HIDDEN,
    }

    with DSStore.open(dst, "w+") as d:
        d.insert(DSStoreEntry(".", "bwsp", bwsp_type, bwsp))
        d.insert(DSStoreEntry(".", "icvp", icvp_type, icvp))
        d.insert(DSStoreEntry(".", "vSrn", vsrn_type, vsrn))
        for name, pos in iloc.items():
            d.insert(DSStoreEntry(name, "Iloc", iloc_type, tuple(pos)))

    print("输出文件自校验：%s" % dst)
    with DSStore.open(dst, "r") as d:
        for e in d:
            code = e.code.decode() if isinstance(e.code, bytes) else e.code
            if code == "Iloc":
                print("   Iloc %-26s (%s, %s)" % (e.filename, e.value[0], e.value[1]))
            elif code == "bwsp":
                print("   bwsp WindowBounds =", e.value.get("WindowBounds"))
            elif code == "icvp":
                has_alias = "backgroundImageAlias" in e.value
                print(
                    "   icvp backgroundType=%s iconSize=%s 含背景alias=%s"
                    % (e.value.get("backgroundType"), e.value.get("iconSize"), has_alias)
                )
                if has_alias or e.value.get("backgroundType") == 2:
                    print("   ⚠️ 仍带背景引用，不是空白窗口", file=sys.stderr)
                    return 1
            else:
                print("   %s = %r" % (code, e.value))
    print("\n完成。构建脚本直接复用这份模板即可，无需样例 dmg、无需 AppleScript、无背景图。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
