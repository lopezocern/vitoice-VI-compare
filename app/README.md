# VitoiceCompare

基于 Tauri 2 的 Windows 本地素材去重 / 清理 / 优化工具，深色界面，15 个工具。

所有扫描与处理逻辑都在本项目的 Rust 代码中实现（不依赖 `czkawka_core`），
交互与结果页形态参考 Czkawka / Krokiet。

## 目录

```
app/
  web/index.html        前端单文件（内联 CSS/JS，无需构建）
  src-tauri/
    src/lib.rs          Tauri 命令、扫描编排、隔离与撤销
    src/scanner.rs      视频收集、抽帧、pHash 分组
    src/images.rs       相似图像扫描与分组
    src/audio.rs        重复音乐指纹与分组
    src/media.rs        ffmpeg/ffprobe 封装、pHash、汉明距离
    src/tools.rs        文件类工具的扫描实现
    src/jobs.rs         视频优化器 / EXIF 移除器作业
    src/cache.rs        缩略图与哈希缓存
    src/config.rs       配置、目录与环境变量
    src/logging.rs      日志与轮转
    src/model.rs        IPC 数据结构
    src/bin/bench.rs    召回基准（bench feature）
  run.ps1 / build-release.ps1 / build-installer.ps1
docs/                   架构与设计文档
design/                 设计原型与截图
tools/krokiet.exe       Czkawka 对照二进制，不参与构建
```

## 运行

前提：

- Rust 工具链（MSVC target）
- WebView2 运行时（Win10/11 自带）
- **ffmpeg / ffprobe 在 PATH 中**：抽帧、探测、音频指纹、EXIF 清理、视频转码都依赖它们。
  程序不内置、也没有启动自检，缺失时会在扫描阶段逐个文件失败。

```powershell
cd app
.\run.ps1              # 开发版（debug），编译快、运行慢
.\build-release.ps1    # 发布版（release），解码性能约为 debug 的 10 倍
.\build-installer.ps1  # 生成 Windows 安装包（NSIS）
```

> ⚠️ debug 构建的解码性能只有 release 的 1/10 左右。日常扫描请用 `build-release.ps1`，
> 产物位于 `G:\Code\videocc-release\release\vitoicecompare.exe`。

## 发布

```powershell
cd app
.\build-installer.ps1
# → G:\Code\videocc-release\release\bundle\nsis\VitoiceCompare_0.1.0_x64-setup.exe
```

- 安装模式：当前用户（`currentUser`），无需管理员权限；
- 静默安装：`VitoiceCompare_0.1.0_x64-setup.exe /S /D=C:\目标目录`；
- 卸载：安装目录下的 `uninstall.exe`。

## 测试

```powershell
cd app\src-tauri
cargo test --lib                                              # 10 个单元测试
cargo run --release --features bench --bin bench -- <目录>    # 召回基准
```

单元测试覆盖：pHash 的确定性与抗压缩噪声、不同图像的区分度、汉明距离、
帧偏移容差、同源合并、时长过滤、日志时间格式。

召回基准：目录内文件名约定为 `<源编号>__<变体>.mp4`，同编号为同源。测试打印
tol × 阈值 的召回/精确矩阵，用于标定参数。历史标定结果（48 文件 / 72 正样本 /
1056 负样本）：tol 20 + 阈值 0.85 → 召回 100%、精确 100%（逐对口径）。

实际分组还会叠加两道条件：时长分桶（±5 秒、比值不超过 1.6）与连续匹配段
（最长连续段 ≥ 6 帧且占全长 35% 以上），因此线上行为比矩阵更宽松。

## 日志

运行日志写在 `%LOCALAPPDATA%\VitoiceCompare\logs\app.log`，超过 2 MB 自动轮转为
`app.1.log`，同时输出到 stderr。设置页「运行日志 → 打开」可直接定位。
扫描、工具执行、作业转码、隔离与清空、未捕获的 panic 都会记录。

## 性能

以下为早期图片扫描实测（30 张 4000×3000 JPEG，共 17 MB，冷扫），
用于说明 debug/release 与并发/采样策略的量级差异：

| 版本 | 耗时 | 单张 |
|---|---|---|
| debug · 串行 · 高质量缩放 | ~120 秒（未跑完） | ~4.3 秒 |
| debug · 并行 · 快速采样缩放 | 15.9 秒 | 0.53 秒 |
| **release · 并行 · 快速采样缩放** | **0.9 秒** | **0.03 秒** |

已落地的优化：

- worker 池并发，默认「可用核心数 / 2」（上限 8），每个 ffmpeg 限 2 线程，避免线程互相抢核；
- 哈希采样用 `thumbnail_exact(32, 32)` 替代高质量缩放；
- 缩略图与哈希落盘缓存（`meta.json`，size + mtime 校验），二次扫描 ≈ 0；
- **两阶段扫描**：第一阶段只算 pHash，第二阶段只为进入分组的文件生成展示帧；
- 重复文件工具的全文件哈希也走 worker 池并发；
- 进度事件 120–200ms 节流，避免大量文件打爆 IPC；取消会直接杀掉正在运行的 ffmpeg。

尚未做的优化：

1. **JPEG 1/8 缩放解码**：按 1/8 尺寸解码取哈希，大图可再快 3–5 倍；
2. 缩略图从 720 降到 480，编码与 IO 更省。

> ⚠️ **不要直接 `cargo run`**。本机工作区目录上残留了沙箱 ACL，WebView2 在该目录下创建
> 渲染进程会返回 `HRESULT 0x800700AA (ERROR_BUSY)`，窗口起不来。`run.ps1` 会把构建产物
> 输出到 `G:\Code\videocc-target`，绕开这个问题。详见文件内注释。

首次编译约 1.5 分钟；之后增量编译 10 秒级。

## 与 Czkawka 的关系

- 结果页交互、工具划分参考 Czkawka / Krokiet，但**代码不依赖 `czkawka_core`，也不复制
  `krokiet` 的任何源码**；15 个工具均为本项目自行实现（见下表）。
- `tools/krokiet.exe` 仅是行为对照用的官方二进制（GPL-3.0），不参与构建、不随安装包分发。

## 当前进度

- [x] **M1 骨架**：窗口 + UI + 键盘流 + IPC 通路（`tool_list`）
- [x] **M2 引擎**：ffmpeg 抽帧 + 64 位 pHash + 并查集分组 + 进度事件 + 真实帧渲染
- [x] **M2.1 召回**：pHash 替代 dHash，时长分桶预筛 + 连续匹配段判定
- [x] **M3 性能**：worker 池并发 + 哈希/缩略图缓存
- [x] **M4 决策**：隔离区、manifest、撤销恢复、批次数统计
- [x] **M5 工具铺开**：15 个工具全部可用（含重复音乐、无声视频与两个执行型工具）
- [ ] 性能优化：JPEG 1/8 解码、缩略图尺寸下调

## 工具实现状态

| 工具 | 实现方式 |
|---|---|
| 重复文件 | 按大小预分组 → blake3 全文件哈希 |
| 空文件夹 | 遍历目录，`read_dir` 为空 |
| 大文件 | 按大小降序取前 500 |
| 空文件 | `size == 0` |
| 临时文件 | 扩展名 `tmp/temp/bak/old/part/crdownload/download` 或 `~` 结尾 |
| 相似图像 | 缩略图 + 64 位 pHash + 低 32 位倒排候选 + 汉明距离 |
| 相似视频 | ffmpeg 抽帧（8–48 帧）+ 每帧 pHash + 时长分桶 + 并查集 |
| 重复音乐 | 8 kHz RMS 包络 + 互相关（±2 秒），相似度 > 95% 且时长差 < 3% |
| 无声视频 | ffprobe 检测音轨；有音轨时首/中/尾抽样 `volumedetect`，峰值 ≤ -50 dB 视为静音 |
| 无效的符号链接 | `symlink_metadata` 成功但目标不存在 |
| 损坏的文件 | 视频跑 ffprobe 校验；图片尝试打开 |
| 错误的扩展名 | `infer` 读魔术字节，与扩展名比对 |
| 不规范名称 | 首尾空格、点号结尾、多重扩展名、控制字符 |
| EXIF 移除器 | `-map_metadata -1 -c copy` 重新封装，输出到 `_clean\` |
| 视频优化器 | 三档预设 + 队列执行，输出到 `_optimized\` |

15 个工具的 `implemented` 均为 true，左侧栏当前没有置灰项。

## 两种结果视图

- **分组视图**（相似视频、相似图像）：左侧分组列表 + 右侧胶片条对比 + 判定依据。
- **平铺视图**（其余 13 个工具）：结果直接以列表呈现，复选框多选，底栏统一执行隔离。
  重复音乐也走平铺视图，条目用「音频组 N · 相似度 X%」标注分组。

## 开发用环境变量

核心扫描：

| 变量 | 作用 |
|---|---|
| `VIDEOCC_AUTOSCAN=<目录>` | 启动即扫描，省去手动填路径 |
| `VIDEOCC_SCOPE=<inc1;inc2>\|<exc1;exc2>` | 预置扫描范围（含/排除，分号分隔，`\|` 前后分组） |
| `VIDEOCC_RECURSIVE=0` | 关闭递归，只扫目录第一层 |
| `VIDEOCC_TOOL=<工具id>` | 指定启动时使用的工具，配合 AUTOSCAN |
| `VIDEOCC_SIM=<50-100>` | 扫描相似度阈值 |
| `VIDEOCC_TOL=<4-32>` | 逐帧 pHash 汉明距离容差（默认 20） |
| `VIDEOCC_CACHE_DIR=<目录>` | 覆盖缓存目录 |
| `VIDEOCC_TRASH_ROOT=<目录>` | 覆盖隔离目录，测试时避免污染真实隔离区 |
| `VIDEOCC_THUMB_SIZE=<240-1600>` | 覆盖缩略图尺寸 |
| `VIDEOCC_WORKERS=<1-32>` | 覆盖并发数 |

界面与自动化（排障/截图用）：

| 变量 | 作用 |
|---|---|
| `VIDEOCC_AUTOCOMPARE=1` | 扫描完成后自动打开对比弹窗 |
| `VIDEOCC_AUTO_TRASH=1` | 扫描完成后自动执行一次「标记删除」 |
| `VIDEOCC_AUTO_TRASH_VIEW=1` | 启动后自动打开隔离区弹窗 |
| `VIDEOCC_OPEN_SETTINGS=1` / `VIDEOCC_OPEN_ABOUT=1` | 启动后自动打开设置 / 关于 |
| `VIDEOCC_OPEN_TRASH=1` | 启动后自动打开隔离区 |
| `VIDEOCC_JOB_TOOL=<id>` / `VIDEOCC_JOB_START=1` | 直接进入作业页并开始执行 |
| `VIDEOCC_DEMO_POS=<0-100>` | 把同步时间轴定位到指定百分比，用于截图验证 |
| `VIDEOCC_SELFTEST=1` | 跑隔离/恢复自检（只操作临时文件），结果写入日志与 stderr |

另有 `VIDEOCC_DIAG_*`、`VIDEOCC_COMPARE`、`VIDEOCC_PREVIEW`、`VIDEOCC_PURGE`
等排障开关，语义见 `src/lib.rs` 的 `dev_flags`。

## 设置页

顶栏「⚙ 设置」打开，配置项：

| 配置 | 默认 | 说明 |
|---|---|---|
| 缓存目录 | `%LOCALAPPDATA%\VitoiceCompare\cache` | 缩略图与哈希缓存，可改到别的盘 |
| 隔离区目录 | `%LOCALAPPDATA%\VitoiceCompare\trash` | 「标记删除」的落点，可改 |
| 缩略图尺寸 | 720 | 相似图像的缩略图；480 更快 / 1080 更清晰（视频展示帧固定 160×90） |
| 并发数 | 自动（核心数/2） | 机械盘建议降到 4 |
| 默认相似度 | 85 | 新扫描的起始阈值 |

面板底部实时显示缓存占用（文件数 + 体积）与隔离区占用，带「清理」与「打开目录」按钮。
配置持久化在 `%LOCALAPPDATA%\VitoiceCompare\config.json`，可用上述环境变量覆盖。

## 执行型工具（优化 / 清理）

点击左侧「视频优化器」或「EXIF 移除器」进入作业页：

1. 用当前扫描范围统计文件（只统计，不处理）；
2. 选择预设（视频）或直接确认（EXIF）；
3. 看预估：体积、耗时、输出位置；
4. 执行 —— 逐个 ffmpeg 处理，实时进度，可随时停止；
5. 完成页给出成功/失败数、节省空间，以及「打开输出目录 / 撤销本次 / 再处理一次」。

视频预设：

| 预设 | 编码 | 质量 | 分辨率上限 | 预估体积比 |
|---|---|---|---|---|
| 社交分享 | H.264 | CRF 23 | 1080p | 约 32% |
| 长期存档 | H.265 | CRF 20 | 原始 | 约 50% |
| 极致压缩 | H.264 | CRF 28 | 720p | 约 24% |

其余约定：

- 原始文件**永不修改**；视频输出 `<原名>_opt.mp4`，EXIF 清理输出 `<原名>_clean.<原扩展名>`；
- 输出目录（源目录下的 `_optimized\` / `_clean\`）带 `manifest.json`，撤销即删除本批次产物；
- `_optimized` / `_clean` / `_backup` 三个目录在扫描时被**自动跳过**，避免把自己的产物扫进来。

## 扫描范围

顶栏「📁 扫描范围」打开浮层，配置包含目录与排除目录：

- 包含目录用 `tauri-plugin-dialog` 的原生选择器，支持多选；路径列表持久化在 localStorage。
- **包含子目录**开关：默认开启（递归扫描）；关闭后只扫所选目录第一层的文件。
- 排除目录优先级高于包含，支持绝对路径与子目录。
- 嵌套包含自动去重（父目录覆盖子目录，不会重复遍历）。
- 包含目录不存在 → 红色标记 + 禁用扫描；排除目录不在任何包含内 → 黄色警告但不阻断。
- 每次启动校验一次路径存在性（`check_paths`）。

## 隔离区设计

- 「标记删除」不是删除，是把文件移动到 `%LOCALAPPDATA%\VitoiceCompare\trash\<批次时间戳>\`。
- 每批写一份 `manifest.json`，记录 `src / dst / size`，恢复时按原路径回填，父目录不存在会自动创建。
- 底栏「撤销」只回滚当前会话的最后一批；跨会话请用「隔离区」弹窗逐批恢复或销毁。
- 被隔离的文件会从分组里剔除；组内不足 2 个文件时该组自动消失。

## 实现要点

- **视频抽帧**：一次 ffmpeg 解码同时产出两路 —— 8–48 帧 160×90 JPEG（展示用）+
  同帧 32×32 灰度数据（算 pHash）。帧数按时长自适应：`clamp(时长/2, 8, 48)`。
- **pHash**：32×32 灰度做 DCT，取低频 8×8 与中位数二值化，得到 64 位哈希。
  分组不比精确分桶，而是按时长分桶后桶内两两比对，允许 ±4 帧时间偏移。
- **音频指纹**：解码为 8 kHz 单声道，按 0.25 秒窗口取 RMS 包络并标准化，
  再用最大互相关（±2 秒）比较；不是 chromaprint。
- **缓存**：`%LOCALAPPDATA%\VitoiceCompare\cache\<path-hash>\meta.json`，
  记录尺寸、修改时间、哈希、帧时间轴；`CACHE_VER` 变更或帧图缺失即失效。
- **保留项**：默认按「分辨率 × 码率 × 时长」评分，最高者预选为保留。

## 已知限制

- 相似视频按时间轴逐帧比对，变速、重新剪辑或大幅调色的版本容易漏检。
- 音频指纹基于响度包络，只能识别同一母带的不同编码/格式，无法识别翻录或混音版本。
- ffmpeg / ffprobe 未内置，安装包的「运行前提」需要用户自行准备。
- 大目录（数千视频）首次扫描仍需较长时间；同一时长桶内候选超过 700 个时只比较邻近窗口，
  极端情况下可能漏掉部分跨窗口的同源文件。
