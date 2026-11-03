# VitoiceCompare 技术方案 v0.1

> 目标：在 Tauri 上重建 Czkawka 的结果页体验，保留 14 个工具的能力。
> 范围：M1–M4。本文档定义架构、接口与里程碑。

---

## 1. 架构总览

```
┌─────────────────────────────────────────────┐
│ WebView2 (前端)                              │
│  React/TS 或原生 JS · 三栏布局 · 胶片条        │
│  invoke("command") ←→ event("scan://progress")│
└───────────────┬─────────────────────────────┘
                │ Tauri IPC
┌───────────────▼─────────────────────────────┐
│ Rust 后端 (src-tauri)                        │
│  ┌───────────────────────────────────────┐  │
│  │ czkawka_core 12.0.2 (MIT)             │  │
│  │  · 重复文件 / 相似图像 / 相似视频 …      │  │
│  └───────────────────────────────────────┘  │
│  ┌───────────────────────────────────────┐  │
│  │ videocc-core（本项目新增）              │  │
│  │  · 抽帧策略器 FrameSampler             │  │
│  │  · 缩略图缓存 ThumbCache               │  │
│  │  · 分组存储 SQLite (write-ahead)        │  │
│  │  · 隔离区 TrashService                 │  │
│  └───────────────────────────────────────┘  │
└─────────────────────────────────────────────┘
```

**分层原则**：`czkawka_core` 只负责"算出结果"，本项目的 `videocc-core` 负责"结果的呈现形态"（抽帧、缩略图、分组索引、隔离）。两者解耦，core 升级不冲击业务层。

---

## 2. 数据模型

```rust
struct ScanRequest {
    paths: Vec<PathBuf>,
    tool: ToolKind,          // SimilarVideos | Duplicates | ...
    similarity: u8,          // 0-100
    frame_policy: FramePolicy,
}

struct FramePolicy {          // 见 §3
    min_frames: u8,           // 默认 8
    max_frames: u8,           // 默认 48
    seconds_per_frame: f32,   // 默认 2.0
}

struct Group {
    id: u64,
    sim: u8,
    file_count: u16,
    total_bytes: u64,
    cover: [PathBuf; 3],      // 列表页三张缩略图
    matched_range: (f32, f32),// 匹配段起止（秒），长视频窗口化用
}

struct GroupDetail {
    group: Group,
    files: Vec<FileEntry>,
    evidence: Evidence,
}

struct FileEntry {
    path: PathBuf,
    resolution: (u32, u32),
    bitrate_kbps: u32,
    duration_secs: f32,
    size: u64,
    keep: bool,
    frames: Vec<FrameRef>,    // 展示帧（8–12 张）
}

struct FrameRef { path: PathBuf, at_secs: f32 }

struct Evidence {
    matched_frames: u16,
    total_frames: u16,
    longest_run: u16,
    audio_match: bool,
    avg_hamming: f32,
}
```

---

## 3. 抽帧策略（FrameSampler）

**匹配帧**（算哈希，不落盘）：
```
frames = clamp(duration / seconds_per_frame, min_frames, max_frames)
interval = duration / frames
```
- 时长 > 10 分钟：额外插入场景切帧（`select='gt(scene,0.3)'`），上限 64。
- 只存 64bit pHash（8 字节/帧）。

**展示帧**（落盘为 160×90 JPEG）：固定 8 张，取值为匹配段的等分点；匹配段未知时取全片等分。
- 缓存路径：`%LOCALAPPDATA%\VitoiceCompare\thumbs\<xxh3(path)>_<i>.jpg`
- 预算：1240 视频 × 8 张 × ~8 KB ≈ 80 MB。

**长视频窗口化**：`matched_range` 来自分组阶段的最长连续匹配段；胶片条只渲染该区间。

---

## 4. IPC 接口

### Commands（前端 → Rust）

| 命令 | 入参 | 返回 | 说明 |
|---|---|---|---|
| `start_scan` | `ScanRequest` | `ScanId` | 异步启动，立即返回 |
| `cancel_scan` | `ScanId` | `()` | 协作式取消 |
| `scan_state` | — | `ScanState` | 断线重连/刷新后恢复 |
| `list_groups` | `{ offset, limit, sort, filter }` | `{ total, items: Group[] }` | 虚拟列表分页 |
| `group_detail` | `{ id }` | `GroupDetail` | 按需加载 |
| `apply_decisions` | `{ ids, action }` | `ActionResult` | 隔离 / 保留 |
| `undo_last` | — | `ActionResult` | 回滚上一批 |
| `tool_list` | — | `ToolMeta[]` | 14 个工具的元信息 |

### Events（Rust → 前端）

| 事件 | Payload |
|---|---|
| `scan://progress` | `{ phase, done, total, eta_secs }` |
| `scan://grouped` | `{ group_count, file_count, elapsed_ms }` |
| `scan://error` | `{ message, recoverable }` |

**关键约束**：`list_groups` 必须分页。178 组 × 每行 3 张缩略图，一次性传输会把 IPC 打爆。

---

## 5. 前端结构

```
app/web/
  index.html          入口（M1 复用原型）
  styles.css
  app.js
    ├── store.js      状态（当前组、选中、过滤）
    ├── api.js        invoke 封装 + mock 开关
    ├── components/
    │    ├── rail      工具导航
    │    ├── groupList 分组列表（虚拟滚动）
    │    ├── detail    详情 + 胶片条
    │    └── compare   全屏对比
    └── keyboard.js   J/K/D/Space/Enter
```

M1 阶段 `api.js` 用 mock 数据跑通 UI；M2 换成真实 invoke，UI 层零改动。

---

## 6. 里程碑

| 阶段 | 内容 | 交付 | 人天 |
|---|---|---|---|
| **M1** | 骨架：Tauri 窗口 + UI 移植 + mock 数据 + 键盘流 | 能点开的窗口 | 3–4 |
| **M2** | 引擎：接入 czkawka_core，真实扫描 + 进度事件 | 能扫真实目录 | 8–10 |
| **M3** | 呈现：抽帧落盘、缩略图缓存、分组详情 | 能看真实胶片条 | 6–8 |
| **M4** | 决策：保留策略、隔离区、撤销、manifest | 能安全清理 | 4–5 |
| **M5** | 收口：14 个工具逐个适配结果页 | 完整替代 Czkawka | 15–20 |

合计约 **36–47 人天**，与前期评估吻合。M1–M4 是核心，M5 是长尾。

---

## 7. 风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| czkawka_core API 面向自家 GUI，非稳定库接口 | M2 适配返工 | 适配层只暴露 `Group/FileEntry`，内部实现可替换 |
| 长视频抽帧耗时长 | 用户以为卡死 | 进度事件细化到"当前文件 + 帧序号" |
| 178 组缩略图内存 | 前端卡顿 | 虚拟滚动 + 图片懒加载 + 磁盘缓存 |
| WebView2 在部分 Win10 缺失 | 启动失败 | 首启检测，缺失时引导安装 |
| 与上游 GPL 前端混淆 | 法律风险 | 只依赖 MIT 的 czkawka_core，不复制 krokiet 代码 |

---

## 8. 本阶段（M1）验收

1. `cargo run` 启动，出现 1440×900 深色窗口，无控制台残留。
2. UI 与 `design/prototype.html` 视觉一致（像素级）。
3. 键盘 J/K/D/Space/Enter 全部生效。
4. `invoke("tool_list")` 返回 14 个工具，证明 IPC 通路可用。
5. 关闭窗口进程完全退出，无孤儿进程。
