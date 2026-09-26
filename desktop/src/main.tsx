import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import appIcon from "../src-tauri/icons/icon.svg";
import { Check, ChevronRight, Github, LoaderCircle, Moon, Plus, RefreshCw, Sun, X } from "lucide-react";
import "./styles.css";

window.addEventListener("error", (event) => {
  const root = document.getElementById("root");
  if (!root || root.dataset.rendered === "true") return;
  root.innerHTML = `<div class="fatal-error"><strong>QM Unlock 加载失败</strong><span>${String(event.error?.message || event.message || "未知前端错误")}</span><small>请从终端运行 npm run tauri dev 查看详细日志</small></div>`;
});
window.addEventListener("unhandledrejection", (event) => {
  const root = document.getElementById("root");
  if (!root || root.dataset.rendered === "true") return;
  root.innerHTML = `<div class="fatal-error"><strong>QM Unlock 加载失败</strong><span>${String(event.reason?.message || event.reason || "未知异步错误")}</span><small>请从终端运行 npm run tauri dev 查看详细日志</small></div>`;
});

type CredentialStatus = {
  available: boolean;
  state: string;
  platform: string;
  account_hint?: string;
  message: string;
};

type FileInfo = {
  path: string;
  supported: boolean;
  kind: "enc" | "plain" | "unknown";
  format?: string;
  songMid?: string;
  resourceFilename?: string;
  error?: string;
};

type DecryptResult = {
  input: string;
  output?: string;
  ok: boolean;
  format?: string;
  error?: string;
  cover?: string;
  lyrics?: string;
};

type JobRow = { tab: Ctx; result: DecryptResult };

type OutputMode = "original" | "mp3";
type KeyMode = "automatic" | "manual";
type Ctx = "enc" | "plain";
type Theme = "dark" | "light";

type ProgressEvent = {
  phase: string;
  input: string;
  current: number;
  total: number;
  /** 整批进度，用于顶栏与底栏 */
  percent: number;
  /** 当前文件自身进度，用于队列行内进度条 */
  filePercent: number;
  message: string;
};

type ScanResult = { files: string[]; infos: FileInfo[] };

const THEME_KEY = "qm-theme";

/// 进度阶段 → 界面上的两字标签。行内只显示阶段，不显示百分比，
/// 百分比属于「整批任务」，放在底栏与顶部进度线上。
const PHASE_LABEL: Record<string, string> = {
  scan: "扫描",
  parse: "读取",
  ekey: "密钥",
  decrypt: "解密",
  transcode: "转码",
  cover: "封面",
  lyrics: "歌词",
  complete: "完成",
};

function basename(path: string) {
  return path.split(/[\\/]/).pop() ?? path;
}

function uniquePaths(values: string[]) {
  return [...new Set(values.filter(Boolean))];
}

function initialTheme(): Theme {
  try {
    const saved = localStorage.getItem(THEME_KEY);
    if (saved === "dark" || saved === "light") return saved;
  } catch {
    /* 隐私模式下 localStorage 不可用，回落到系统偏好 */
  }
  return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

export default function App() {
  const [paths, setPaths] = useState<string[]>([]);
  const [fileInfo, setFileInfo] = useState<Record<string, FileInfo>>({});
  const [tab, setTab] = useState<Ctx>("enc");
  const [outputDir, setOutputDir] = useState("");
  const [lyricsDir, setLyricsDir] = useState("");
  const [mode, setMode] = useState<OutputMode>("original");
  const [keyMode, setKeyMode] = useState<KeyMode>("automatic");
  const [manualKey, setManualKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [credentials, setCredentials] = useState<CredentialStatus>();
  const [os, setOs] = useState("");
  const [running, setRunning] = useState(false);
  const [jobs, setJobs] = useState<JobRow[]>([]);
  const [notice, setNotice] = useState<string>();
  const [progress, setProgress] = useState<ProgressEvent>();
  const [dragActive, setDragActive] = useState(false);
  const [embedCover, setEmbedCover] = useState(true);
  const [fetchLyrics, setFetchLyrics] = useState(false);
  const [embedSyncedLyrics, setEmbedSyncedLyrics] = useState(false);
  const [plainCopy, setPlainCopy] = useState(false);
  const [theme, setTheme] = useState<Theme>(initialTheme);
  const [noticeBad, setNoticeBad] = useState(false);
  const permissionSettingsOpened = useRef(false);

  useEffect(() => {
    const root = document.getElementById("root");
    if (root) root.dataset.rendered = "true";
  }, []);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    try {
      localStorage.setItem(THEME_KEY, theme);
    } catch {
      /* 忽略隐私模式下的写入失败 */
    }
  }, [theme]);

  // macOS 改用 overlay 标题栏：原生红绿灯叠进自绘顶栏，
  // 并清空系统标题，避免「系统标题一行 + 自绘品牌字一行」的重复。
  useEffect(() => {
    if (os !== "macos") return;
    const win = getCurrentWindow();
    win.setTitleBarStyle("overlay").catch(() => {});
    win.setTitle("").catch(() => {});
  }, [os]);

  // 提示 6 秒自动消失，避免堆积遮挡界面
  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(undefined), 6000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  useEffect(() => {
    invoke<string>("os_platform")
      .then(setOs)
      .catch(() => setOs(""));
  }, []);

  const refreshCredentials = useCallback(async (requestPermission = false) => {
    try {
      const status = await invoke<CredentialStatus>("check_credentials");
      setCredentials(status);
      // macOS 无法让应用自行授予完全磁盘访问权限；首次检测到缺失时，
      // 自动打开准确的系统设置页面，授权开关仍由用户亲自确认。
      if (requestPermission && status.state === "permission_required" && !permissionSettingsOpened.current) {
        permissionSettingsOpened.current = true;
        void invoke("open_full_disk_access_settings").catch(() => {});
      }
    } catch {
      setCredentials({ available: false, state: "unavailable", platform: "unknown", message: "无法读取登录状态" });
    }
  }, []);

  useEffect(() => {
    void refreshCredentials(true);
  }, [refreshCredentials]);

  const openFullDiskAccessSettings = useCallback(() => {
    void invoke("open_full_disk_access_settings").catch(() => {
      setNotice("无法打开系统设置，请前往「隐私与安全性 → 完全磁盘访问权限」手动允许 QM Unlock");
      setNoticeBad(true);
    });
  }, []);

  // 窗口重新获得焦点时自动刷新登录态。
  // 顶栏原本有个手动刷新按钮，但用户在外部登录 QQ 音乐后并不知道要点它，
  // 按钮也解释不清自己干什么，于是改成聚焦即刷新，按钮移除。
  useEffect(() => {
    const onFocus = () => {
      void refreshCredentials();
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refreshCredentials]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<ProgressEvent>("decrypt-progress", (event) => setProgress(event.payload)).then(
      (stop) => {
        unlisten = stop;
      },
    );
    return () => unlisten?.();
  }, []);

  const scanIncoming = useCallback(async (incoming: string[]) => {
    const candidates = uniquePaths(incoming);
    if (!candidates.length) return;
    setNotice(undefined);
    setNoticeBad(false);
    setProgress({ phase: "scan", input: "", current: 0, total: 1, percent: 0, filePercent: 0, message: "" });
    try {
      const result = await invoke<ScanResult>("scan_paths", { paths: candidates });
      if (!result.files.length) {
        setNotice("没有可处理的音频文件");
        setNoticeBad(true);
        return;
      }
      setPaths((old) => uniquePaths([...old, ...result.files]));
      setFileInfo((old) => ({
        ...old,
        ...Object.fromEntries(result.infos.map((info) => [info.path, info])),
      }));
    } catch (error) {
      setNotice(`扫描失败：${String(error)}`);
      setNoticeBad(true);
    }
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") setDragActive(true);
        else if (event.payload.type === "leave") setDragActive(false);
        else if (event.payload.type === "drop") {
          setDragActive(false);
          void scanIncoming(event.payload.paths);
        }
      })
      .then((stop) => {
        unlisten = stop;
      })
      .catch(() => {
        /* 开发工具里退回 HTML5 拖放 */
      });
    return () => unlisten?.();
  }, [scanIncoming]);

  const chooseFiles = async () => {
    const picked = await open({
      multiple: true,
      filters: [
        { name: "加密音频", extensions: ["mgg", "mflac", "mmp4"] },
        { name: "普通音频", extensions: ["flac", "mp3", "m4a", "ogg", "opus", "wav"] },
      ],
    });
    if (Array.isArray(picked)) void scanIncoming(picked);
    else if (typeof picked === "string") void scanIncoming([picked]);
  };

  const chooseFolder = async () => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") void scanIncoming([picked]);
  };

  const chooseOutput = async () => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setOutputDir(picked);
  };

  const chooseLyricsDir = async () => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setLyricsDir(picked);
  };

  const handleDrop = (event: React.DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setDragActive(false);
    const dropped = Array.from(event.dataTransfer.files)
      .map((file) => (file as File & { path?: string }).path)
      .filter((path): path is string => Boolean(path));
    if (dropped.length) void scanIncoming(dropped);
  };

  const removePath = (path: string) => {
    setPaths((old) => old.filter((item) => item !== path));
    setFileInfo((old) => {
      const next = { ...old };
      delete next[path];
      return next;
    });
  };

  const clearAll = () => {
    setPaths([]);
    setFileInfo({});
    setJobs([]);
    setNotice(undefined);
    setNoticeBad(false);
    setProgress(undefined);
  };

  const encPaths = useMemo(
    () => paths.filter((path) => fileInfo[path]?.kind === "enc"),
    [paths, fileInfo],
  );
  const plainPaths = useMemo(
    () => paths.filter((path) => fileInfo[path]?.kind === "plain"),
    [paths, fileInfo],
  );
  const hasBoth = encPaths.length > 0 && plainPaths.length > 0;
  const ctx: Ctx = hasBoth ? tab : encPaths.length > 0 ? "enc" : "plain";
  const visiblePaths = ctx === "enc" ? encPaths : plainPaths;
  const isMac = os === "macos";

  const start = async () => {
    if (!visiblePaths.length || running) return;
    if (ctx === "enc") {
      if (keyMode === "manual" && !manualKey.trim()) {
        setNotice("请粘贴 ekey，或切换为自动获取");
        setNoticeBad(true);
        return;
      }
      if (keyMode === "automatic" && !credentials?.available) {
        setNotice(credentials?.message || "正在检测 QQ 音乐登录信息，请稍后重试");
        setNoticeBad(true);
        return;
      }
    }
    setNotice(undefined);
    setNoticeBad(false);
    setRunning(true);
    setJobs((old) => old.filter((row) => row.tab !== ctx));
    setProgress(undefined);
    try {
      const result = await invoke<DecryptResult[]>("decrypt_paths", {
        paths: visiblePaths,
        outputDir: outputDir || null,
        options: {
          output_mode: mode,
          manual_ekey: ctx === "enc" && keyMode === "manual" ? manualKey.trim() : null,
          embed_cover: embedCover,
          fetch_lyrics: fetchLyrics,
          lyrics_dir: fetchLyrics && lyricsDir ? lyricsDir : null,
          embed_synced_lyrics: fetchLyrics && embedSyncedLyrics,
          plain_copy: ctx === "plain" ? plainCopy : false,
        },
      });
      setJobs((old) => [...old, ...result.map((r) => ({ tab: ctx, result: r }))]);
      const ok = result.filter((r) => r.ok).length;
      const fail = result.length - ok;
      setNotice(fail > 0 ? `${ok}/${result.length} 完成 · ${fail} 失败` : `${ok}/${result.length} 完成`);
      setNoticeBad(fail > 0);
    } catch (error) {
      setJobs((old) => [
        ...old,
        { tab: ctx, result: { input: "任务", ok: false, error: String(error) } },
      ]);
      setNotice(`任务失败：${String(error)}`);
      setNoticeBad(true);
    } finally {
      setRunning(false);
      void refreshCredentials();
    }
  };

  // ⌘O 添加文件、⌘↩ 开始：让顶栏的快捷键提示是真的，而不是装饰
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!event.metaKey && !event.ctrlKey) return;
      const key = event.key.toLowerCase();
      if (key === "o") {
        event.preventDefault();
        void chooseFiles();
      } else if (key === "enter") {
        event.preventDefault();
        void start();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const keyReady = ctx === "plain" ? true : keyMode === "manual" ? manualKey.trim().length > 0 : Boolean(credentials?.available);
  const canStart = visiblePaths.length > 0 && keyReady && !running;
  const state = dragActive ? "drag" : paths.length ? "loaded" : "empty";
  const permissionRequired = credentials?.state === "permission_required";
  const tabJobs = jobs.filter((row) => row.tab === ctx);
  const failedJob = tabJobs.find((row) => !row.result.ok)?.result;
  const jobByPath = useMemo(() => {
    const map = new Map<string, DecryptResult>();
    for (const row of tabJobs) map.set(row.result.input, row.result);
    return map;
  }, [tabJobs]);

  return (
    <main className={`shell${isMac ? " is-mac" : ""}${hasBoth ? " has-both" : ""}`} data-state={state}>
      <header className="topbar" data-tauri-drag-region>
        <div className="wm">
          <img src={appIcon} alt="" />
          <span className="wm-name">QM Unlock</span>
        </div>
        <div
          className="tools"
          // 顶栏是拖拽区；按钮区域阻止 mousedown 冒泡，避免点按钮时误触发窗口拖动
          onMouseDown={(event) => event.stopPropagation()}
        >
          <button
            className="ico-btn"
            title="切换深浅色"
            aria-label="切换深浅色"
            onClick={() => setTheme((t) => (t === "dark" ? "light" : "dark"))}
          >
            {theme === "dark" ? <Sun size={14} /> : <Moon size={14} />}
          </button>
          <a
            className="ico-btn"
            href="https://github.com/mary20050520/qmunlock"
            title="GitHub"
            aria-label="在 GitHub 打开 QM Unlock"
            onClick={(event) => {
              // WKWebView 不保证处理 target="_blank"，交给原生层用系统浏览器打开。
              event.preventDefault();
              void invoke("open_external_url", { url: "https://github.com/mary20050520/qmunlock" });
            }}
          >
            <Github size={14} />
          </a>
        </div>
        <div className="topline" style={{ width: running && progress ? `${progress.percent}%` : 0 }} />
      </header>

      <div className="body">
        <section
          className="queue"
          onDragOver={(event) => {
            event.preventDefault();
            setDragActive(true);
          }}
          onDragLeave={(event) => {
            // 只有真正离开队列区域才收起遮罩，移到子元素上不算离开，避免遮罩闪烁
            if (!event.currentTarget.contains(event.relatedTarget as Node)) setDragActive(false);
          }}
          onDrop={handleDrop}
        >
          <div className="qtabs" role="tablist" aria-label="队列类型">
            <button className="qtab" role="tab" aria-selected={ctx === "enc"} onClick={() => setTab("enc")}>
              加密<b>{encPaths.length}</b>
            </button>
            <button className="qtab" role="tab" aria-selected={ctx === "plain"} onClick={() => setTab("plain")}>
              普通<b>{plainPaths.length}</b>
            </button>
          </div>

          {paths.length === 0 ? (
            <div className="drop">
              <div className="halo">
                <img src={appIcon} alt="" />
              </div>
              <p>{dragActive ? "松开以添加" : "拖入音频"}</p>
              <div className="fmts">
                .mflac .mgg .mmp4<span className="sep">|</span>.flac .mp3 .m4a .ogg
              </div>
              <div className="acts">
                <button className="ghost" onClick={() => void chooseFiles()}>选择文件</button>
                <button className="ghost" onClick={() => void chooseFolder()}>文件夹</button>
              </div>
              <section className={`auth-guide${credentials?.available ? " ready" : ""}${permissionRequired ? " required" : ""}`} aria-label="自动解密状态与使用说明">
                <div className="auth-guide-head">
                  <i />
                  <span>自动获取 ekey</span>
                </div>
                <strong>
                  {credentials?.available
                    ? `已就绪${credentials.account_hint ? ` · ${credentials.account_hint}` : ""}`
                    : permissionRequired
                      ? "需要完全磁盘访问权限"
                      : credentials?.state === "qqmusic_not_found"
                        ? "尚未找到 QQ 音乐"
                        : credentials
                          ? "尚未读取到登录信息"
                          : "正在检测 QQ 音乐登录状态…"}
                </strong>
                <p>{credentials?.message || "首次启动会自动检查 QQ 音乐登录状态。"}</p>
                {permissionRequired && (
                  <>
                    <ol>
                      <li>在「完全磁盘访问权限」页点击左下角 <b>+</b>。</li>
                      <li>选择「应用程序」中的 <b>QM Unlock.app</b>。</li>
                      <li>打开 QM Unlock 右侧开关，再返回本应用自动重新检测。</li>
                    </ol>
                    <button className="guide-action" onClick={openFullDiskAccessSettings}>打开系统设置</button>
                  </>
                )}
                <small>若刚重新登录 QQ 音乐，请回到本应用刷新状态；仍无法获取 ekey 时可改用手动 ekey。</small>
              </section>
            </div>
          ) : (
            <>
              <div className="qhead">
                <span>queue</span>
                <b>{visiblePaths.length}</b>
                <button
                  className="add"
                  title="添加文件"
                  aria-label="添加文件"
                  onClick={() => void chooseFiles()}
                >
                  <Plus size={12} />
                </button>
                <button className="clear" onClick={clearAll}>clear</button>
              </div>
              <div className="qlist">
                {visiblePaths.map((path) => {
                  const info = fileInfo[path];
                  const active = running && progress?.input === path;
                  const job = jobByPath.get(path);
                  const bad = info?.supported === false || job?.ok === false;
                  const detail = job
                    ? [job.cover, job.lyrics, job.error].filter(Boolean).join(" · ")
                    : "";
                  return (
                    <div className={`qrow${active ? " on" : ""}`} key={path}>
                      <i className={`st${active ? " w" : bad ? " b" : info ? "" : " idle"}`} />
                      <span className="nm" title={detail || path}>{basename(path)}</span>
                      <span className={`pc${job?.ok ? " ok" : bad ? " err" : ""}`}>
                        {active
                          ? PHASE_LABEL[progress?.phase ?? ""] ?? "处理"
                          : job
                            ? job.ok
                              ? job.format?.toUpperCase() || "OK"
                              : "失败"
                            : info?.supported === false
                              ? "不可用"
                              : info?.format?.toUpperCase() || "…"}
                      </span>
                      {active && <span className="rbar" style={{ width: `${progress?.filePercent ?? 0}%` }} />}
                      <button className="rm" title="移除" aria-label={`移除 ${basename(path)}`} onClick={() => removePath(path)}>
                        <X size={13} />
                      </button>
                    </div>
                  );
                })}
              </div>
            </>
          )}
          <div className="veil">
            <p>松开以添加</p>
          </div>
        </section>

        <aside className="side">
          {ctx === "enc" && (
            <div className="card">
              <p className="ch">key</p>
              <div className="seg" role="tablist" aria-label="ekey 来源">
                <button aria-pressed={keyMode === "automatic"} onClick={() => setKeyMode("automatic")}>自动</button>
                <button aria-pressed={keyMode === "manual"} onClick={() => setKeyMode("manual")}>手动</button>
              </div>
              {keyMode === "automatic" ? (
                <>
                  <div className="keyrow">
                    <i className={credentials?.available ? "" : " bad"} />
                    <span className="mono">{credentials?.available ? credentials.account_hint || "已登录" : credentials ? "未读取到登录信息" : "检测中…"}</span>
                    <button className="key-refresh" title="重新检测登录状态" aria-label="重新检测登录状态" onClick={() => void refreshCredentials()}>
                      <RefreshCw size={13} />
                    </button>
                  </div>
                  {credentials && !credentials.available && <p className="key-error">{credentials.message}</p>}
                  {failedJob?.error && <p className="key-error">解密失败：{failedJob.error}</p>}
                </>
              ) : (
                <div className="keyfield">
                  <input
                    value={manualKey}
                    onChange={(event) => setManualKey(event.target.value)}
                    type={showKey ? "text" : "password"}
                    spellCheck={false}
                    placeholder="base64 ekey"
                  />
                  <button onClick={() => setShowKey((v) => !v)}>{showKey ? "隐藏" : "显示"}</button>
                </div>
              )}
            </div>
          )}

          {ctx === "enc" && (
            <div className="card">
              <p className="ch">format</p>
              <div className="seg" role="tablist" aria-label="输出格式">
                <button aria-pressed={mode === "original"} onClick={() => setMode("original")}>原格式</button>
                <button aria-pressed={mode === "mp3"} onClick={() => setMode("mp3")}>MP3</button>
              </div>
            </div>
          )}

          {ctx === "plain" && (
            <div className="card">
              <p className="ch">write</p>
              <div className="seg" role="tablist" aria-label="写入方式">
                <button aria-pressed={!plainCopy} onClick={() => setPlainCopy(false)}>原位</button>
                <button aria-pressed={plainCopy} onClick={() => setPlainCopy(true)}>副本</button>
              </div>
            </div>
          )}

          <div className="card">
            <p className="ch">enhance</p>
            <button className="opt first" aria-pressed={embedCover} onClick={() => setEmbedCover((v) => !v)}>
              <span className="box"><Check size={9} strokeWidth={4} /></span>
              <span>封面</span>
            </button>
            <button className="opt" aria-pressed={fetchLyrics} onClick={() => setFetchLyrics((v) => !v)}>
              <span className="box"><Check size={9} strokeWidth={4} /></span>
              <span>歌词</span>
            </button>
            {fetchLyrics && (
              <button className="opt subopt" aria-pressed={embedSyncedLyrics} onClick={() => setEmbedSyncedLyrics((v) => !v)}>
                <span className="box"><Check size={9} strokeWidth={4} /></span>
                <span>嵌入同步歌词（实验性）</span>
              </button>
            )}
          </div>

          <div className="card">
            <p className="ch">output</p>
            {(ctx === "enc" || plainCopy) && (
              <button className="path" style={{ marginTop: 0 }} onClick={() => void chooseOutput()} title={outputDir || "与源文件相同"}>
                <span className="chip">out</span>
                <span className="pv">{outputDir || "同源目录"}</span>
                <ChevronRight size={13} />
              </button>
            )}
            {fetchLyrics && (
              <button className="path lrc" onClick={() => void chooseLyricsDir()} title={lyricsDir || "与音频同目录"}>
                <span className="chip">lrc</span>
                <span className="pv">{lyricsDir || "同音频目录"}</span>
                <ChevronRight size={13} />
              </button>
            )}
          </div>

        </aside>
      </div>

      <footer className="foot">
        <div className="count">
          {running ? `${progress?.percent ?? 0}%` : visiblePaths.length}
          <small>{running ? "处理中" : ctx === "enc" ? "待解密" : "待写入"}</small>
        </div>
        <button className="go" disabled={!canStart} onClick={() => void start()}>
          {running ? <LoaderCircle size={14} className="spin" /> : null}
          <span>{running ? "处理中" : ctx === "enc" ? "解密" : "写入"}</span>
          <kbd>↩</kbd>
        </button>
      </footer>

      {notice && (
        <div className={`toast${noticeBad ? " bad" : ""}`} role="status">
          <i />
          <span>{notice}</span>
        </div>
      )}
    </main>
  );
}

const rootElement = document.getElementById("root");
if (!rootElement) {
  throw new Error("QM Unlock 找不到应用挂载节点");
}
createRoot(rootElement).render(<App />);
