# VitoiceCompare

基于 Tauri 2 + `czkawka_core` 的视频素材去重/整理工具。保留 Czkawka 的 14 个工具能力，重做结果呈现层。

## 目录

```
app/
  web/                 前端（静态 HTML/CSS/JS，无需构建）
    index.html         主界面
  src-tauri/           Rust 后端
    src/lib.rs         Tauri 命令与生命周期
    src/main.rs        入口
    tauri.conf.json    窗口与打包配置
    icons/             应用图标
docs/
  ARCHITECTURE.md      架构、接口、里程碑
design/
  prototype.html       设计原型（可交互）
  DESIGN.md            设计说明
```

## 运行

前提：Rust 工具链（MSVC）+ WebView2 运行时（Win10/11 自带）。

```powershell
cd app
.\run.ps1              # 开发版（debug），编译快、运行慢
.\build-release.ps1    # 发布版（release），编译稍慢、运行快 10 倍以上
.\build-installer.ps1  # 生成 Windows 安装包（NSIS）
```

> ⚠️ debug 构建的解码性能只有 release 的 1/10 左右。日常扫描请用 `build-release.ps1`，
> 它会把产物放到 `G:\Code\videocc-release\release\vitoicecompare.exe`。

## 发布

```powershell
cd app
.\build-installer.ps1
# → G:\Code\videocc-release\release\bundle\nsis\VitoiceCompare_0.1.0_x64-setup.exe
```

- 安装模式：当前用户（`currentUser`），无需管理员权限；
- 安装包约 2.3 MB（应用本体 9.2 MB，LZMA 压缩）；
- 静默安装：`VitoiceCompare_0.1.0_x64-setup.exe /S /D=C:\目标目录`；
- 卸载：安装目录下的 `uninstall.exe`。

## 测试

```powershell
cd app\src-tauri
cargo test --lib                          # 10 个单元测试
cargo run --release --features bench --bin bench -- <目录>   # 召回基准
```

单元测试覆盖：pHash 的确定性与抗噪性、汉明距离、帧偏移对齐、分组并查集、时长过滤、日志时间格式。

召回基准：目录内文件名约定为 `<源编号>__<变体>.mp4`，同编号为同源。测试会打印
tol × 阈值 的召回/精确矩阵，用于标定参数。当前标定结果（48 文件 / 72 正样本 / 1056 负样本）：
tol 20 + 阈值 0.85 → 召回 100%、精确 100%。

## 日志

运行日志写在 `%LOCALAPPDATA%\VitoiceCompare\logs\app.log`，超过 2 MB 自动轮转为 `app.1.log`。
设置页「运行日志 → 打开」可直接定位。扫描、工具执行、作业转码、隔离与清空、未捕获的 panic 都会记录。

## 性能


实测：30 张 4000×3000 JPEG（共 17 MB），冷扫耗时。

| 版本 | 耗时 | 单张 |
|---|---|---|
| debug · 串行 · 高质量缩放 | ~120 秒（未跑完） | ~4.3 秒 |
| debug · 并行 · 快速采样缩放 | 15.9 秒 | 0.53 秒 |
| **release · 并行 · 快速采样缩放** | **0.9 秒** | **0.03 秒** |

其他已有优化：
- 8 路并发（16 核取一半），图片与视频共用同一套 worker 池策略；
- `thumbnail_exact` 替代 `resize_exact(Triangle)` 做哈希采样，快一个量级；
- 缩略图与哈希落盘缓存，按 size + mtime 校验，二次扫描 ≈ 0。

尚未做的优化（按收益排序）：
1. **两阶段**：先只算哈希，缩略图仅对进入分组的文件生成 —— 重复率低时再省 30–40%；
2. **JPEG 1/8 缩放解码**：按 1/8 尺寸解码取哈希，大图可再快 3–5 倍；
3. 缩略图从 720 降到 480，编码与 IO 更省。

> ⚠️ **不要直接 `cargo run`**。本机工作区目录上残留了沙箱 ACL，WebView2 在该目录下创建
> 渲染进程会返回 `HRESULT 0x800700AA (ERROR_BUSY)`，窗口起不来。`run.ps1` 会把构建产物
> 输出到 `G:\Code\videocc-target`，绕开这个问题。详见文件内注释。

首次编译约 1.5 分钟；之后增量编译 10 秒级。

## 与 Czkawka 的关系


- 只依赖 `czkawka_core`（MIT），不包含/不复制 `krokiet`（GPL-3.0）的任何代码。
- 本项目自身的许可证可自由选择。
- Czkawka 二进制仅作为对照参考，位于 `tools/krokiet.exe`，不参与构建。

## 当前进度

- [x] **M1 骨架**：窗口 + UI + 键盘流 + IPC 通路（`tool_list`）
- [x] **M2 引擎**：真实扫描（ffmpeg 抽帧 + dHash）+ 分组 + 进度事件 + 真实帧渲染
- [x] **M3 性能**：8 路并发 + 哈希缓存（冷扫 1236 文件 4 分 23 秒，热扫 0.1 秒）
- [x] **M4 决策**：隔离区、manifest、撤销恢复、批次数统计
- [x] 搜索过滤、三种排序、文件路径显示
- [ ] M3.1 召回：dHash → pHash + 时长预筛（当前召回仅 123/772，待修）
- [x] **M5 工具铺开**：10 个工具可用，4 个待实现（见下表）

## 工具实现状态

| 工具 | 状态 | 实现方式 |
|---|---|---|
| 重复文件 | ✅ | 按大小预分组 → blake3 全文件哈希 |
| 空文件夹 | ✅ | 遍历目录，read_dir 为空 |
| 大文件 | ✅ | 按大小排序取前 500 |
| 空文件 | ✅ | size == 0 |
| 临时文件 | ✅ | 扩展名 + `~` 结尾 |
| 相似视频 | ✅ | ffmpeg 抽帧 + dHash + 并查集分组 |
| 无效的符号链接 | ✅ | symlink_metadata 成功但目标不存在 |
| 损坏的文件 | ✅ | 媒体文件跑 ffprobe 校验 |
| 错误的扩展名 | ✅ | infer 读魔术字节，与扩展名比对 |
| 不规范名称 | ✅ | 首尾空格、点号结尾、多重扩展名、控制字符 |
| 相似图像 | ✅ | 720px 缩略图 + 64bit dHash + band 倒排分组 |
| 重复音乐 | ⬜ | 需要音频指纹（chromaprint），尚未实现 |
| EXIF 移除器 | ✅ | ffmpeg `-map_metadata -1` 重新封装，输出到 `_clean\\` |
| 视频优化器 | ✅ | 三档预设 + 队列执行，输出到 `_optimized\\` |

未实现的工具在左侧栏置灰，点击会提示「还没实现」。

## 两种结果视图

- **分组视图**（相似视频）：左侧分组列表 + 右侧胶片条对比 + 判定依据。
- **平铺视图**（其余 9 个工具）：结果直接以列表呈现，复选框多选，底栏统一执行隔离。

## 开发用环境变量

| 变量 | 作用 |
|---|---|
| `VIDEOCC_AUTOSCAN=<目录>` | 启动即扫描，省去手动填路径 |
| `VIDEOCC_SCOPE=<inc1;inc2>\|<exc1;exc2>` | 预置扫描范围（含/排除，分号分隔，`\|` 前后分组） |
| `VIDEOCC_TOOL=<工具id>` | 指定启动时使用的工具，配合 AUTOSCAN |
| `VIDEOCC_SIM=<50-100>` | 扫描相似度阈值（默认 90） |
| `VIDEOCC_AUTOCOMPARE=1` | 扫描完成后自动打开对比弹窗 |
| `VIDEOCC_DEMO_POS=<0-100>` | 把同步时间轴定位到指定百分比，用于截图验证 |
| `VIDEOCC_AUTO_TRASH=1` | 扫描完成后自动执行一次「标记删除」，用于验证隔离流程 |
| `VIDEOCC_AUTO_TRASH_VIEW=1` | 启动后自动打开隔离区弹窗 |
| `VIDEOCC_SELFTEST=1` | 跑隔离/恢复的单元自检（只操作临时文件），结果打到 stderr |
| `VIDEOCC_TRASH_ROOT=<目录>` | 覆盖隔离目录，测试时避免污染真实隔离区 |

## 设置页

顶栏「⚙ 设置」打开，配置项：

| 配置 | 默认 | 说明 |
|---|---|---|
| 缓存目录 | `%LOCALAPPDATA%\VitoiceCompare\cache` | 缩略图与哈希缓存，可改到别的盘 |
| 隔离区目录 | `%LOCALAPPDATA%\VitoiceCompare\trash` | 「标记删除」的落点，可改 |
| 缩略图尺寸 | 720 | 480 更快 / 1080 更清晰，影响扫描速度与磁盘占用 |
| 并发数 | 自动（核心数/2） | 机械盘建议降到 4 |
| 默认相似度 | 90 | 新扫描的起始阈值 |

面板底部实时显示缓存占用（文件数 + 体积），带「清理」与「打开目录」按钮。
配置持久化在 `%LOCALAPPDATA%\VitoiceCompare\config.json`，可用 `VIDEOCC_CACHE_DIR` / `VIDEOCC_TRASH_ROOT` 等环境变量覆盖。

## 执行型工具（优化 / 清理）

点击左侧「视频优化器」或「EXIF 移除器」进入作业页：

1. 用当前扫描范围统计文件（只统计，不处理）；
2. 选择预设（视频）或直接确认（EXIF）；
3. 看预估：体积、耗时、输出位置；
4. 执行 —— 逐个 ffmpeg 处理，实时进度，可随时停止；
5. 完成页给出成功/失败数、节省空间，以及「打开输出目录 / 撤销本次 / 再处理一次」。

安全约束：
- 原始文件**永不修改**，输出写到源目录的 `_optimized\\` 或 `_clean\\`；
- 输出目录带 `manifest.json`，撤销即删除本批次产物；
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
- 底栏「撤销」只回滚当前会话的最后一批；跨会话请用「隔离区」弹窗逐批恢复。
- 被隔离的文件会从分组里剔除；组内不足 2 个文件时该组自动消失。

## 已实现的技术选择

- **自己实现视频抽帧管线**，而不是直接用 `czkawka_core` 的相似视频模块。原因：czkawka 只输出分组结果，不暴露每帧哈希、匹配区间和帧图，而结果页的核心恰恰是这些。`czkawka_core` 留给其余 13 个工具。
- 一次 ffmpeg 解码同时产出两路：8–48 帧 160×90 JPEG（展示用）+ 同帧的 9×8 灰度原始数据（算 dHash）。避免二次解码。
- 分帧数按时长自适应：`frames = clamp(时长/2, 8, 48)`。

## 已知限制（M2 范围）

- ~~哈希未落盘缓存，重复扫描会重新抽帧~~ → M3 已解决：每个视频缓存 `meta.json`（size + mtime 校验），命中即跳过解码。
- 分组采用「同索引帧比对」，头尾被裁剪的视频覆盖率会偏低。
- 音频指纹未接入，判定依据栏显示「未检测」。

## M3：并发与缓存

- **并发**：worker 池，默认 `可用核心数 / 2`（上限 8），每个 ffmpeg 限 2 线程，避免 8×N 线程互相抢核。
- **缓存**：`%LOCALAPPDATA%\VitoiceCompare\cache\<path-hash>\meta.json`，记录尺寸、修改时间、哈希、帧时间轴；帧图仍在同目录。
- **进度节流**：120ms 一次事件，避免 1240 个文件打爆 IPC。
- **取消**：`AtomicBool` 在每个 worker 的文件循环里检查。
