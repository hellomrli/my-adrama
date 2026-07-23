# AI 短剧生产工作流 — Rust 跨平台应用设计方案

> 交付形式：本文档为设计方案，代码由用户自行编写。

## Context

目标：搭建一条「剧本 → 资产（角色/服化道）→ 分镜图 → 视频」的 AI 短剧生产流水线。
- 图像生成：OpenAI `gpt-image-2`（角色定妆照、服装、道具、场景、分镜图）
- 视频生成：Google 视频模型（用户称 "omni"；**注意**：截至已知信息，Google 的视频生成模型是 **Veo 3 / Veo 3.1**，经 Gemini API 调用，并无 "Omni" 视频模型。建议实现时以 https://ai.google.dev/gemini-api/docs/video 官方文档为准，模型 ID 做成配置项）
- 语言/平台：Rust，Linux + Windows
- 流程控制：**分阶段人工确认**——每阶段产物落盘，人工审核/重生成满意后才进入下一阶段（视频生成很贵，避免浪费额度）

## 总体架构

CLI + 桌面 GUI（egui/eframe；无子命令时启动 GUI），以「项目目录」为核心的状态机流水线：

```
adrama new <project>          # 初始化项目
adrama import script.md       # 导入剧本
adrama parse                  # 阶段1: LLM 解析剧本 → 结构化 JSON
adrama assets [--only 角色名] # 阶段2: 生成角色/服化道资产图
adrama storyboard [--scene N] # 阶段3: 生成分镜图
adrama video [--shot N]       # 阶段4: 分镜 → 视频片段
adrama status                 # 查看各阶段状态
adrama approve <stage>        # 标记阶段通过，解锁下一阶段
adrama redo <stage> --id X    # 重生成某个单项
```

### 项目目录结构（即持久化状态）

```
my-drama/
├── project.toml          # 项目配置（风格、画幅、模型 ID 覆盖）
├── script/               # 原始剧本
├── parsed/
│   └── breakdown.json    # 结构化剧本：角色表、场景表、镜头表
├── assets/
│   ├── characters/<name>/   # 定妆照多角度图 + prompt.txt + meta.json
│   ├── costumes/  props/  locations/
├── storyboard/
│   └── s01_shot03.png + s01_shot03.json (prompt、关联资产、审核状态)
├── video/
│   └── s01_shot03.mp4 + .json (任务 id、状态)
└── state.json            # 各阶段审核状态（pending/approved）
```

状态全部落盘为可读文件 → 用户可直接手改 prompt/图片后继续，天然支持断点续跑。

## 四个阶段的设计

### 阶段 1：剧本解析（LLM）
- 调 LLM（可用 OpenAI chat API，同一把 key）把剧本解析为结构化 `breakdown.json`：
  - `characters[]`: 姓名、外貌描述、服装、性格
  - `scenes[]`: 场景描述、时间、地点
  - `shots[]`: 每场分镜头——景别、机位、画面内容、台词/音效、时长（≤8s，对齐 Veo 单次生成上限）
- 用 JSON Schema / structured output 保证可解析。

### 阶段 2：资产生成（gpt-image-2）
- 对每个角色生成定妆照（正面/侧面/全身，同一 prompt 加视角后缀），服装、道具、场景各生成参考图。
- prompt 模板 = 项目风格前缀（project.toml 里的 style，如「电影感、写实、2.35:1」）+ 资产描述。
- 每个资产的 prompt 存 `prompt.txt`，`redo` 命令允许改后重生成。

### 阶段 3：分镜图生成（gpt-image-2 图像编辑/参考图能力）
- 关键点：**角色一致性**。用 gpt-image-2 的多图输入/编辑端点，把该镜头涉及的角色定妆照 + 场景参考图作为输入图，prompt 描述镜头构图。
- 输出画幅与视频一致（16:9 横屏或 9:16 竖屏短剧，project.toml 配置）。

### 阶段 4：视频生成（Google Veo，image-to-video）
- 每个 shot：分镜图作首帧 + 镜头运动/表演/台词 prompt → 调 Veo image-to-video。
- Veo 是异步长任务：提交后轮询 operation 直到完成，下载 mp4。记录任务 id 到 json，支持中断后恢复轮询。
- 可选收尾：用 ffmpeg（系统依赖或 `ffmpeg-sidecar` crate）按镜头顺序拼接成片。

## Rust 技术选型

| 用途 | crate |
|---|---|
| CLI | `clap` (derive) |
| 异步运行时 + HTTP | `tokio` + `reqwest` (rustls，利于 Windows 交叉编译) |
| 序列化 | `serde` / `serde_json` / `toml` |
| 错误处理 | `anyhow` + `thiserror` |
| 图片 base64 | `base64` |
| 进度显示 | `indicatif` |
| 密钥 | 环境变量 `OPENAI_API_KEY` / `GEMINI_API_KEY`（`dotenvy` 支持 .env） |

API 客户端不建议依赖第三方 SDK crate（更新滞后），直接用 reqwest 手写两个薄客户端：
- `openai.rs`: `POST /v1/images/generations`、`POST /v1/images/edits`（多参考图）、`POST /v1/chat/completions`（剧本解析）
- `google.rs`: Gemini API `models/veo-3.x:predictLongRunning` + operation 轮询 + 文件下载。**模型 ID 从 project.toml 读取**，方便切换新模型。

## 模块划分建议

```
src/
├── main.rs          # clap 命令分发
├── project.rs       # 项目目录、project.toml、state.json 读写
├── model/           # breakdown.json 等数据结构
├── api/openai.rs    # 图像 + LLM 客户端
├── api/google.rs    # 视频客户端（长任务轮询）
├── stages/parse.rs / assets.rs / storyboard.rs / video.rs
└── pipeline.rs      # 阶段门控：上一阶段未 approve 则拒绝执行
```

## 实施顺序

1. 项目骨架：clap 命令 + project.toml/state.json 读写 + `new`/`status`
2. 阶段 1 剧本解析（最便宜，先打通 LLM 调用与数据模型）
3. 阶段 2 资产生成（打通 gpt-image-2）
4. 阶段 3 分镜（多参考图编辑端点，重点调角色一致性）
5. 阶段 4 视频（Veo 长任务轮询 + 下载）
6. 收尾：`redo`/`approve` 门控完善、ffmpeg 拼接、Windows 构建验证（`cargo build --target x86_64-pc-windows-gnu` 或直接 Windows 下构建；用 rustls 避免 OpenSSL 依赖）

## 验证方式

- 用一个 2 场景、4 镜头的迷你剧本走全流程冒烟测试。
- 每阶段先用 `--dry-run`（只打印将要发送的 prompt，不实际调 API）验证 prompt 组装，再小规模真调。
- 视频阶段先只跑 1 个 shot 验证轮询/下载逻辑，再批量。

## 风险与注意

- **模型名待查证**：`gpt-image-2` 与 Google "omni" 均以实现当日官方文档为准；本方案已把模型 ID 全部做成配置项，改名零成本。
- **角色一致性**是成片质量的最大风险点，阶段 3 值得预留最多的调试时间（多参考图 + 详细外貌描述冗余写入 prompt）。
- 视频生成成本高，务必保留阶段门控与单 shot 粒度的 redo。
