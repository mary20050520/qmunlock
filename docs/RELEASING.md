# Releasing QM Unlock

## Prerequisites

- The `main` branch passes the Verify workflow.
- Update `desktop/src-tauri/tauri.conf.json`, `desktop/package.json`, and `CHANGELOG.md` with the release version.
- Review `git status` and confirm that no credentials, ekeys, captures, test music, or local build output are included.

## Unsigned release

1. Create and push a tag in the form `vX.Y.Z`.
2. The **Release desktop app** workflow builds macOS arm64 (Apple Silicon), macOS x64 (Intel), and Windows x64 packages.
3. It creates a GitHub Release and attaches the `.dmg`, `.exe`, and `.msi` files.
4. Test each target package on a clean machine before marking the release as stable.

The default workflow ad-hoc signs the completed macOS app, verifies the app and nested code, then creates the DMG. It requires no secrets, does not identify a developer, and does not replace Developer ID signing or notarization; macOS may still require the user to choose “Open Anyway”.

## Optional signing

- **macOS:** use a Developer ID Application certificate and notarize the finished application with Apple.
- **Windows:** sign the installer and executable with an Authenticode certificate. An EV certificate can improve SmartScreen reputation over time.

Do not put certificate files, passwords, Apple credentials, ekeys, or QQ Music credentials in the repository. Store release credentials only as protected GitHub Actions secrets.

## 样式化 DMG（空白窗口 + 图标布局）

CI 与本地共用同一个打包引擎 `desktop/scripts/make_dmg_styled.sh`：

```bash
bash desktop/scripts/make_dmg_styled.sh \
  "desktop/src-tauri/target/release/bundle/macos/QM Unlock.app" \
  "dist/QM Unlock_1.0.0_aarch64.dmg"
```

CI 侧由 `desktop/scripts/sign-and-package-macos.sh <rust-target>` 解析 target、版本号与产物路径后委托给它，因此两条路径的签名、校验、布局完全一致。

脚本不依赖 GUI，可在无头环境（含 CI runner）运行。七步流程：暂存 → 清 xattr 并 ad-hoc 签名、逐个校验嵌套代码 → `makehybrid -hfs` 转可读写映像 → 挂载到 `/tmp` 自定义挂载点设卷图标并清 FinderInfo → 转只读压缩 UDZO → 清理临时文件 → 只读回读校验。

### 三条铁律

1. **布局一律来自 `desktop/scripts/dmg-template.DS_Store`，绝不用 AppleScript / osascript 指挥 Finder。** 无 GUI 或沙箱环境下 AppleScript 会失败、超时或静默不生效。
2. **不做背景图。** 空白窗口跟随系统外观（浅色系统黑标签、深色系统白标签都可读），也彻底避开「背景白底 / 背景被拉伸错位 / alias 与书签解析失败」这一整类问题。曾经尝试过深色与浅色两版背景图，都因为访达的窗口尺寸恢复机制和标签配色问题被否掉，最终决定就是空白。
3. **挂载一律用 `-mountpoint /tmp/xxx`，绝不用默认的 `/Volumes/<名字>`。**

### 布局模板的来历

`dmg-template.DS_Store` 由 `dmg_layout.py` 从一份 Finder 认可的样例 `.DS_Store` 派生：只替换条目名、把图标尺寸调到 84、删掉背景引用（`backgroundType` 置 0、不写 `backgroundImageAlias` 与 `pBBk`），窗口 bounds 等字节原样保留：

```bash
python3 desktop/scripts/dmg_layout.py <样例.DS_Store> \
  desktop/scripts/dmg-template.DS_Store "QM Unlock.app" "① 安装与故障排查.txt"
```

改布局请改 `dmg_layout.py` 重新生成，不要手改模板。当前坐标：app (180,190)、
Applications (480,190)、说明文件右上角 (592,70)、隐藏文件推到窗口可视区外。

### 卷名不能随便改

访达会按**卷名**恢复「上次的窗口大小」，一旦恢复就忽略 `.DS_Store` 里的
`WindowBounds`，窗口尺寸就不受控（有背景图时还会表现为背景拉伸错位）。
所以卷名固定为 `QM Unlock Setup`（一个从未被打开过的名字），而不是 app 名。
模板本身不含卷名信息，改卷名不需要重新生成模板。

### 踩过的坑

| 症状 | 根因 | 处理 |
| --- | --- | --- |
| 背景图白底 / 错位，反复修不好 | `dmgbuild` 只写老式 `backgroundImageAlias`（324 字节残缺），缺新版 Finder 优先使用的 `pBBk` 书签；且访达按卷名恢复窗口大小后会把背景拉伸、与绝对坐标的图标错位 | 最终决定不做背景图：`backgroundType=0`、不写 alias 与书签，空白窗口 |
| 窗口比 660×400 大、布局「歪」 | 访达按**卷名**恢复上次窗口大小，恢复后忽略 `WindowBounds`；背景图被拉伸而图标坐标是绝对像素 | 卷名用从未打开过的 `QM Unlock Setup`；无背景图后即使窗口被改也只是留白变化 |
| `codesign --verify --strict` 报 detritus not allowed | `makehybrid -hfs` 会给 `.icns` 和可执行文件写入 HFS 文件类型/creator，挂载后表现为 20 个 `com.apple.FinderInfo` 扩展属性 | 在卷内对 app 执行 `xattr -cr`；只清 app，卷根和 `.VolumeIcon.icns` 的 FinderInfo 是自定义卷图标所依赖的，不能动 |
| Tauri 产物直接校验失败 | 构建产物签名不完整（`code has no resources but signature indicates they must be present`） | 打包前 `xattr -cr` 再 ad-hoc 重签 |
| 卷图标是默认磁盘图标 | 只放 `.VolumeIcon.icns` 不设属性 | `SetFile -c icnC` 与 `SetFile -a C` 两步都要；缺 SetFile 时脚本只告警不中断 |

