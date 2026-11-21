# VitoiceCompare 技术方案

> 适用于 v0.1.0 的当前实现。
> 早期方案曾计划复用 `czkawka_core`，最终全部能力改为自研；本文与代码保持一致。

---

## 1. 架构总览

```
┌──────────────────────────────────────────────┐
│ WebView2（app/web/index.html，单文件）        │
│  顶栏 / 工具导航 / 结果列表或分组 / 详情       │
│  invoke(cmd)  ←→  listen(scan://, job://)     │
└──────────────────┬───────────────────────────┘
                   │ Tauri IPC
┌──────────────────▼───────────────────────────┐
│ Rust（src-tauri/src）                        │
│  lib.rs       命令、状态、线程调度、隔离区     │
│  scanner.rs   视频抽帧与 pHash 分组           │
│  images.rs    相似图像                        │
│  audio.rs     重复音乐                        │
│  media.rs     ffmpeg / ffprobe 封装、pHash    │
│  tools.rs     9 个文件类工具                  │
│  jobs.rs      视频优化器 / EXIF 移除器        │
│  cache.rs     缩略图与哈希缓存                │
│  config.rs    配置与目录                      │
│  logging.rs   日志与轮转                      │
│  model.rs     IPC 数据结构                    │
└──────────────────┬───────────────────────────┘
                   │ 子进程 / 文件系统
     ffmpeg / ffprobe · 素材目录 · %LOCALAPPDATA%\VitoiceCompare
```

**分层原则**

- 命令层（`lib.rs`）只做参数校验、范围签名和线程调度，不写扫描算法；
- 每个工具产出独立的 `ToolSnapshot`，按工具 ID 缓存在内存里，切换工具不重新扫描；
- 结果不落数据库：内存里有快照，磁盘上只有缩略图/哈希缓存和隔离/作业 manifest。

---

## 2. 状态与数据模型

```rust
struct AppState {
    results: Mutex<HashMap<String, ToolSnapshot>>, // 按工具缓存结果
    scope_sig: Mutex<String>,                      // 当前扫描范围签名
    tool: Mutex<String>,                           // 当前工具
    trashed: Mutex<HashSet<String>>,               // 本次会话已隔离的路径
    scanning: AtomicBool,
    cancel: Arc<AtomicBool>,                       // 扫描取消
    job_running: AtomicBool,
    job_cancel: Arc<AtomicBool>,                   // 作业取消
}

struct ToolSnapshot {
    records: Vec<VideoRecord>,     // 相似视频/图像
    groups: Vec<(Vec<usize>, u8)>, // 成员下标 + 相似度
    items: Vec<Item>,              // 平铺结果
    sig: String,                   // 产生结果时的范围签名
}

struct VideoRecord {
    path: PathBuf,
    meta: Probe,          // 宽高、时长、码率、大小
    hashes: Vec<u64>,     // 每帧 pHash
    frames: Vec<PathBuf>, // 缓存中的展示帧
    times: Vec<f32>,      // 每帧时间点
}

struct GroupSummary { id, sim, file_count, total_bytes, covers, names }
struct GroupDetail  { group, files: Vec<FileEntry>, evidence }
struct FileEntry    { path, name, width, height, bitrate, duration, size, keep, frames }
struct Evidence     { matched_frames, total_frames, longest_run, avg_hamming }
struct Item         { path, name, size, detail, kind }
```

`Probe`、`VideoRecord` 由 `media::probe` / `scanner` 产出；`GroupSummary` 等是发给前端的
视图模型，定义在 `model.rs`。作业侧另有 `JobFile / JobPreview / JobManifest`（`jobs.rs`），
缓存侧有 `CachedVideo`（`cache.rs`，当前 `CACHE_VER = 2`）。

---

## 3. 扫描流程

### 3.1 相似视频

1. **收集**：多根目录 + 排除集，嵌套包含自动去重；跳过 `_optimized` / `_clean` / `_backup`，
   以及非视频扩展名。
2. **第一阶段 · 哈希**：worker 池（默认核心数/2，上限 8）并发处理。每个文件先查缓存
   （`size + mtime` 命中 `meta.json` 即跳过解码），未命中则一次 ffmpeg 解码只产出
   32×32 灰度数据，计算 64 位 pHash。帧数 `clamp(时长 / 2, 8, 48)`，等间隔采样。
   此阶段不写展示图，未进入分组的文件到此为止。
3. **分组**：按时长 5 秒分桶（相邻桶合并，候选池上限 700），桶内两两比对；
   超过 700 时按时长排序后只比较邻近窗口，不再整桶跳过。时长比 > 1.6 直接跳过。
   逐帧比较允许 ±4 帧偏移，汉明距离容差默认 20（`VIDEOCC_TOL`）。整段匹配率 ≥ 阈值，
   或最长连续匹配段 ≥ 6 帧且占 35% 以上，即用并查集合并。
4. **第二阶段 · 展示帧**：只为分组内的文件重新解码生成 160×90 JPEG，写回缓存；
   失败数与第一阶段失败数一起计入 `scan://finished` 的 `failed`。
5. **输出**：组相似度取成员与首成员匹配率的平均值；结果按相似度、成员数排序。

### 3.2 相似图像

- 按扩展名收集，生成设置页指定尺寸（默认 720）的缩略图，同时把原图缩到 32×32 灰度算 pHash；
- 用哈希低 32 位和次 32 位建倒排桶，桶内按汉明距离合并（容差由默认相似度换算）；
- 其它流程与视频一致（缓存、并查集、按组排序）。

### 3.3 重复音乐

- ffmpeg 解码为 8 kHz 单声道 s16le，按 0.25 秒窗口计算 RMS 包络并做标准化；
- 按时长分桶（±1 秒，时长比 ≤ 1.03，池上限 400，超过后按相近时长窗口比较），用最大互相关（±2 秒）比较；
- 相关系数 > 0.95 合并为同一组；
- 音频结果不进入分组视图，而是展开为平铺条目，`detail` 里标注「音频组 N · 相似度 X%」。

### 3.4 文件类工具

`walk.rs` 是统一的遍历入口（多根目录、嵌套去重、排除与保留目录、取消检查），
四个扫描模块共用；`tools.rs` 在 `run` 中分发：

| 工具 | 判据 |
|---|---|
| duplicates | 先按大小分组，再对同大小文件并发做 blake3 全文件哈希 |
| biggest-files | 按大小降序取前 500 |
| empty-files | `size == 0` |
| empty-folders | `read_dir` 为空 |
| temporary | 扩展名白名单或 `~` 结尾 |
| invalid-symlinks | `symlink_metadata` 成功但目标不存在 |
| bad-extensions | `infer` 魔术字节与扩展名比对 |
| broken-files | 视频跑 ffprobe，图片尝试打开 |
| bad-names | 首尾空格、点号结尾、多重扩展名、控制字符 |

---

## 4. IPC 接口

Tauri 2 会把前端传入的 camelCase 参数映射到 Rust 的 snake_case 参数。

### Commands

| 分类 | 命令 | 入参 → 返回 |
|---|---|---|
| 元信息 | `tool_list` | — → `ToolMeta[]`（15 个工具 + `implemented`） |
| | `ping` / `app_version` / `dev_flags` | — → 字符串 / JSON |
| 扫描 | `scan_start` | `path, sim?, tool?` → `()` |
| | `scan_start_scope` | `includes, excludes, sim?, tool?, recursive?` → `()` |
| | `scan_cancel` / `is_scanning` | — → `()` / `bool` |
| | `check_paths` | `paths` → `bool[]` |
| | `pick_folders` | — → `string[]`（原生目录选择器） |
| 结果 | `get_groups` | `tool?` → `GroupSummary[]` |
| | `group_detail` | `id, tool?` → `GroupDetail` |
| | `get_items` | `tool?` → `Item[]` |
| | `has_result` / `get_tool` | `tool` / — → `bool` / `string` |
| 媒体 | `file_data_url` / `frame_data_url` | `path` / `path, at, width?` → `data:` URL |
| | `open_path` / `open_in_explorer` | `path` → `()` |
| 决策 | `trash_files` | `paths` → `{batch, moved, bytes}` |
| | `undo_batch` / `purge_batch` / `purge_all` | `batch` / — → JSON |
| | `list_batches` / `trash_stats` | — → JSON |
| 作业 | `job_preview` | `tool, includes, excludes, recursive?` → `JobPreview` |
| | `job_start` | `tool, preset, files, firstRoot` → `()` |
| | `job_cancel` / `job_finish` / `job_undo` | — / `outDir` → `()` / `usize` |
| 配置 | `get_config` / `set_config` | — / `cfg` → `AppConfig` |
| | `cache_stats` / `clear_cache` / `open_logs` | — → JSON / JSON / 路径 |

### Events

| 事件 | Payload |
|---|---|
| `scan://progress` | `{ done, total, current }` |
| `scan://finished`（视频/图像） | `{ groups, files, matched_files, elapsed_ms }` |
| `scan://finished`（文件类工具） | `{ tool, groups: 0, files, matched_files, elapsed_ms }` |
| `job://progress` | `{ done, total, current, pct }` |
| `job://finished` | `{ ok, failed, errors, in_bytes, out_bytes, out_dir, elapsed_ms, cancelled }` |

没有独立的 `scan://error` 事件：命令直接返回 `Result<_, String>`，扫描内部错误写日志并跳过
单个文件。

**约束**：结果快照只保存在内存里，进程退出即丢失；再次扫描同一范围会命中磁盘缓存，
几百个文件通常在一秒内完成。

---

## 5. 前端结构

`app/web/index.html` 是唯一的页面文件，CSS 与 JS 全部内联，不使用构建工具。

```
顶栏         品牌 / 当前工具 / 搜索 / 扫描范围 / 开始扫描 / 设置 / 关于 / 窗口按钮
进度条       扫描或作业进行时展开
工具导航     基础 / 媒体 / 高级三组，未实现项置灰（当前无）
主区域       平铺列表 或 分组列表；右侧详情 + 胶片条 + 判定依据
作业页       配置 → 预估 → 执行 → 报告（视频优化器 / EXIF 移除器）
弹层         扫描范围、设置、隔离区、关于、全屏对比、图片查看
```

前端状态集中在少量全局变量：`ITEMS`、`GROUPS`、`VIEW`、`currentTool`、`selected`、
`SCOPE`、`APP_CFG`、`JOB`。键盘流：`J/K` 切换分组、`D` 标记删除、`Space` 播放/暂停、
`Enter` 打开对比、`Esc` 关闭弹层。

---

## 6. 缓存、隔离与作业

**缓存**：`%LOCALAPPDATA%\VitoiceCompare\cache\<path-hash>\`，含 `meta.json` 与帧图。
`cache::load_meta` 只校验版本号、`size + mtime` 和哈希，供第一阶段复用；
`cache::load` 额外要求帧图存在，供需要展示图的场景使用。
缓存目录可在设置页修改或用 `VIDEOCC_CACHE_DIR` 覆盖。

**隔离区**：`trash\<批次毫秒时间戳>\`，批次内 `manifest.json` 记录 `src / dst / size`。
「标记删除」是移动文件；`undo_batch` 按 manifest 回填原路径并删除批次目录；
`purge_batch` / `purge_all` 才是真正删除。被隔离的文件会从内存快照的分组中剔除。

**作业**：输出到扫描根目录下的 `_optimized\` 或 `_clean\`，每次执行写 `manifest.json`
（`src / out / in_bytes / out_bytes`）。撤销只删除本批次产物并移除 manifest，不碰原文件。

---

## 7. 并发与性能

- worker 池：`config::workers()`，默认 `可用核心数 / 2`，上限 8；
- 每个 ffmpeg 子进程固定 `-threads 2`，避免 8×N 线程互相抢核；
- 两阶段扫描：第一阶段只算哈希，第二阶段只为分组内文件生成展示帧；
- 重复文件工具的 blake3 全文件哈希同样走 worker 池；
- 进度事件节流：视频 120ms、图像 150ms、展示帧 200ms 一次，避免大量文件打爆 IPC；
- 取消：`AtomicBool` 在文件循环里检查，`media::output_timeout_cancel` 每 50ms 检查一次并杀掉
  正在运行的 ffmpeg；
- 子进程超时：抽帧 180 秒、ffprobe 30 秒、转码 3600 秒，坏文件不会卡死整个扫描。

发布版构建关闭 LTO 以缩短构建时间（运行性能差异在 5% 以内），详见 `build-release.ps1`。

---

## 8. 里程碑

| 阶段 | 内容 | 状态 |
|---|---|---|
| **M1** | 骨架：窗口 + UI + 键盘流 + IPC 通路 | ✅ |
| **M2** | 引擎：ffmpeg 抽帧 + pHash + 分组 + 进度事件 | ✅ |
| **M2.1** | 召回：pHash 替代 dHash，时长分桶 + 连续匹配段 | ✅ |
| **M3** | 并发与缓存：worker 池 + `meta.json` 缓存 | ✅ |
| **M4** | 决策：隔离区、manifest、撤销、批次统计 | ✅ |
| **M5** | 工具铺开：15 个工具全部可用 | ✅ |
| **M6** | 性能：两阶段扫描、重复文件并发哈希 | ✅ |
| **M7** | 性能：JPEG 1/8 解码缩放、缩略图尺寸下调 | ⬜ |

---

## 9. 风险与对策

| 风险 | 影响 | 现状 / 对策 |
|---|---|---|
| ffmpeg / ffprobe 不在 PATH | 大部分工具无法工作 | 未内置、未自检；README 已列为运行前提 |
| 大目录首次扫描慢 | 用户以为卡死 | 并发 + 缓存 + 两阶段 + 进度节流 |
| 桶内两两比对是 O(n²) | 超大目录耗时陡增 | 视频/音频超限后改为时长邻近窗口；图像桶上限 20000，超限写日志 |
| 变速 / 重剪版本漏检 | 召回不足 | 连续匹配段判定缓解；仍无法覆盖变速 |
| 结果全在内存 | 数千文件占用较大内存 | 只保留必要字段与帧路径，缩略图按需转 data URL 并做 LRU 式缓存 |
| WebView2 在工作区目录下起不来 | 窗口无法创建 | `run.ps1` 把 `CARGO_TARGET_DIR` 放到工作区外 |

---

## 10. 测试

```powershell
cd app\src-tauri
cargo test --lib                                              # 10 个单元测试
cargo run --release --features bench --bin bench -- <目录>    # 召回基准
```

单元测试覆盖 pHash、汉明距离、帧偏移容差、并查集分组、时长过滤与日志时间格式。
`VIDEOCC_SELFTEST=1` 可在启动时自检隔离/恢复流程（只操作临时文件）。
