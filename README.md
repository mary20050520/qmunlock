<p align="center">
  <img src="desktop/src-tauri/icons/icon.svg" width="104" alt="QM Unlock logo">
</p>

<h1 align="center">QM Unlock</h1>

<p align="center"><strong>把新版 QQ 音乐下载的 musicex 文件，在本机解出可播放的音频。</strong></p>

<p align="center">
  <img src="https://img.shields.io/badge/macOS-Apple%20Silicon%20%7C%20Intel-151515?logo=apple&logoColor=white" alt="macOS Apple Silicon and Intel">
  <img src="https://img.shields.io/badge/Windows-x64-0078D4?logo=windows&logoColor=white" alt="Windows x64">
  <img src="https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white" alt="Tauri 2">
  <img src="https://img.shields.io/badge/License-MIT-60B932" alt="MIT license">
</p>

<p align="center">
  <img src="docs/assets/qmunlock-desktop-macos.png" width="880" alt="QM Unlock macOS 界面">
</p>

<p align="center"><sub>macOS 界面示意。拖入文件、选择输出方式，即可开始处理。</sub></p>

QM Unlock 是一个本地运行的 Rust + Tauri 桌面工具，用于处理带 **musicex V1 footer** 的 QQ 音乐下载文件。它支持批量拖放、自动或手动 ekey、原格式输出与 MP3 转换，也能为 MP3 补齐封面、歌词和基础音乐标签。

> 仅处理你有权访问的本地文件。项目不提供音乐下载、帐号登录或付费内容绕过功能。

## 快速开始

1. 从 [Releases](../../releases) 下载适合自己电脑的安装包，将 macOS App 拖进「应用程序」文件夹后再打开。
2. 在 QQ 音乐中登录，并下载你有权访问的 `.mgg`、`.mflac` 或 `.mmp4` 文件。
3. 将文件或整个下载文件夹拖进 QM Unlock；也可点「选择文件」或「文件夹」。
4. 选择保留原格式，或转换为 MP3；按需要开启封面和歌词。
5. 点击开始。完成后，文件会写到所选目录（未选时与源文件同目录）。

建议先用一首歌确认流程，再处理整个文件夹。

## macOS：首次自动获取 ekey 的授权步骤

QM Unlock 会读取 QQ 音乐的本地登录信息来自动获取 ekey。macOS 保护这部分数据，因此第一次可能显示「需要完全磁盘访问权限」。

系统设置会自动打开，但 **QM Unlock 不会自动出现在列表里**。请按下面的步骤手动添加一次：

1. 在「系统设置 → 隐私与安全性 → 完全磁盘访问权限」页，点击左下角 **+**。
2. 选择「应用程序」中的 **QM Unlock.app**。
3. 在列表里打开 QM Unlock 右侧的开关。
4. 回到 QM Unlock；窗口重新获得焦点时会自动检测登录状态。

以后正常更新同一个 App 通常会保留这个授权。若列表中找不到它，请确认你已把 App 从 DMG 拖进「应用程序」文件夹，而不是直接在 DMG 内运行。

> macOS 不允许普通应用自行把自己加入「完全磁盘访问权限」列表或替用户打开开关；这一步必须由用户确认。

## 新版功能

| 功能 | 说明 |
| --- | --- |
| 批量处理 | 支持拖入多个文件或整个文件夹，队列会显示逐文件状态与整批进度。 |
| 自动 / 手动 ekey | 默认尝试读取当前 QQ 音乐登录状态；失败时可直接粘贴你自己取得的 ekey。 |
| 原格式或 MP3 | `.mgg`、`.mflac`、`.mmp4` 可分别输出 OGG、FLAC、M4A，也可选择转为 MP3。 |
| 普通音频增强 | `.flac`、`.mp3`、`.m4a`、`.ogg`、`.opus`、`.wav` 可直接补充封面和歌词，无需再次解密。 |
| 封面与标签 | MP3 会写入歌名、歌手、专辑、专辑艺人（数据可用时）、年份、曲序、碟号及正面封面。 |
| 歌词 | 可生成同名 `.lrc`；「嵌入同步歌词（实验性）」会额外写入 MP3 的 `SYLT` 标签，默认关闭。 |
| 安全的普通音频写入 | 可选原地更新或生成副本；副本不会覆盖源文件或同名已有文件。 |
| 深浅主题 | 自动跟随系统偏好，也可在右上角切换；旁边的 GitHub 图标会使用系统默认浏览器打开项目主页。 |

### 关于封面、歌词与标签

- 封面和歌词来自 QQ 音乐公开曲目信息的匹配结果；匹配不到时会跳过，不会强行写入疑似错误的信息。
- 开启「歌词」后，同名带时间轴的 `.lrc` 会保留在音频旁（或你选定的歌词目录）。这是车机和多数第三方播放器最实用的方案。
- `SYLT` 是 ID3 同步歌词标准，但并非所有播放器支持；Apple Music 对本地文件的滚动歌词支持尤其不稳定，所以它是实验性开关，不默认写入。
- 给 MP3 补封面和文字标签只改写标签区域，**不会重新编码 MP3 音频**。

## 兼容性与实测基线

QQ 音乐客户端的内部存储会变化；下表是已验证的基线，而不是对所有历史版本的承诺。

| 平台 | 已验证 QQ 音乐版本 | 已验证输入 | 结果 |
| --- | --- | --- | --- |
| macOS | `11.8.1` | `.mgg` / `.mflac` / `.mmp4`（musicex V1） | 可自动读取登录状态并完成解密、原格式输出与 MP3 转换。 |
| Windows | `22.5.2` | `.mgg` / `.mflac` / `.mmp4`（musicex V1） | 可完成解密与转换；自动读取依赖 QQ 音乐进程及相同权限级别。 |

当前开发分支已在 macOS Apple Silicon、macOS Intel 和 Windows x64 上完成构建验证。QQ 音乐更新后若自动获取失败，优先重新登录客户端并刷新状态；仍不行时改用手动 ekey。

### 支持与不支持的格式

| 范围 | 说明 |
| --- | --- |
| 支持的加密输入 | 带 `musicex V1 footer` 的 `.mgg`、`.mflac`、`.mmp4`。 |
| 支持的普通音频 | `.flac`、`.mp3`、`.m4a`、`.ogg`、`.opus`、`.wav`，用于封面和歌词增强。 |
| 不支持 | 旧 QMC 体系：`.qmc*`、`.bkcmp3`、QTag、STag，以及任何不带 musicex V1 footer 的文件。 |

文件扩展名不是唯一依据；应用会读取文件实际 footer。下载未完成、文件损坏，或 ekey 与资源不匹配时，无法得到可用的输出。

## 常见问题

<details>
<summary><strong>macOS 里提示需要完全磁盘访问权限，但设置列表没有 QM Unlock</strong></summary>

这是正常情况。点击 `+`，从「应用程序」选择 `QM Unlock.app`，再打开它右侧的开关。不要直接在 DMG 中运行 App；先拖进「应用程序」文件夹。

</details>

<details>
<summary><strong>重新登录 QQ 音乐后，还是无法自动获取 ekey</strong></summary>

回到 QM Unlock 后它会自动刷新状态；确认 QQ 音乐仍保持登录。若提示仍存在，直接切换到手动 ekey 模式，粘贴与当前文件匹配的 ekey。

</details>

<details>
<summary><strong>Windows 找到 QQ 音乐但读取不到登录信息</strong></summary>

确认 QQ 音乐正在运行且已登录；如果 QQ 音乐以管理员身份运行，QM Unlock 也需要以相同权限级别运行。客户端内部实现可能随版本变化，手动 ekey 是稳定的后备方案。

</details>

<details>
<summary><strong>Apple Music 没有显示新封面或没有滚动歌词</strong></summary>

Apple Music 可能缓存了旧文件信息。删除资料库中的旧条目时选择「保留文件」，然后通过「文件 → 导入」重新导入处理后的文件。它可读取普通内嵌歌词，但本地文件的滚动歌词不保证支持；车机和其他播放器优先使用同名 `.lrc`。

</details>

<details>
<summary><strong>macOS 提示无法验证开发者，或 Windows 显示未知发布者</strong></summary>

当前开发包使用 macOS ad-hoc 签名，Windows 包未使用 Authenticode 签名。请只从本仓库下载；确认来源后按系统提示继续打开。面向正式分发时仍建议使用 Developer ID 公证和 Authenticode 签名。

</details>

## 隐私与安全

- 解密、转码和标签写入均在本机进行。
- 自动 ekey 仅为当前任务读取 QQ 音乐本地登录状态；手动 ekey 只保留在当前运行内存，不写入任务历史、日志或配置文件。
- 不要在 Issue、PR、截图或日志中提交 ekey、`authst`、QQ 号、数据库、plist、进程内存或真实音乐文件。
- 请阅读 [免责声明](DISCLAIMER.md) 与 [安全说明](SECURITY.md)。

## 开发与验证

前置条件：Node.js 20+、Rust stable，以及对应平台的 Tauri 系统依赖。

```bash
cd desktop
npm ci
npm run tauri dev
```

```bash
cd desktop
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

发布与 CI 说明见 [docs/RELEASING.md](docs/RELEASING.md)，历史变更见 [CHANGELOG.md](CHANGELOG.md)。欢迎贡献兼容性、体验和测试改进；提交前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 许可

项目代码采用 [MIT License](LICENSE)。随包 FFmpeg / libmp3lame 按其自身 LGPL 条款分发，许可和源码说明位于 [`desktop/src-tauri/resources/ffmpeg`](desktop/src-tauri/resources/ffmpeg)。
