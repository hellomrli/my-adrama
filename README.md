# my-adrama

AI 短剧生产工作流：**剧本 → 资产 → 分镜 → 视频**。  
支持 **图形界面（GUI）** 与 **命令行（CLI）**，Linux / Windows。

仓库：https://github.com/hellomrli/my-adrama

## 下载（云编译产物）

GitHub Actions 会在推送与 Release 时构建：

- **Windows**：`adrama.exe` / `adrama-windows-x86_64.zip`
- **Linux**：`adrama_*.deb`、`adrama-linux-x86_64`

安装 deb：

```bash
sudo dpkg -i adrama_0.1.0_amd64.deb
# 若缺依赖：
sudo apt-get install -f
```

## 依赖

- Rust 1.75+
- 环境变量：`OPENAI_API_KEY`、`GEMINI_API_KEY`
- 可选：系统 `ffmpeg`（视频阶段 `--concat` 拼接成片）

## 安装

```bash
cargo install --path .
# 或
cargo build --release
```

### Windows 发布包（从 Linux 交叉编译）

需要 [llvm-mingw](https://github.com/mstorsjo/llvm-mingw) 与 Rust target `x86_64-pc-windows-gnullvm`：

```bash
rustup target add x86_64-pc-windows-gnullvm
# 将 llvm-mingw 的 bin 加入 PATH 后：
./scripts/package-windows.sh
```

产物在 `dist/`（含 `adrama.exe`）。双击即可打开 GUI。

## 图形界面

无参数启动即打开 GUI（Windows 下双击 `adrama.exe` 同样如此）：

```bash
adrama
# 或
adrama gui
adrama --gui --project ./my-drama
```

界面功能（简体中文）：

| 区域 | 说明 |
|------|------|
| 工作流画布 | 类似 ComfyUI 的节点图：剧本→解析→资产→分镜→视频→成片，可拖拽/缩放/运行/审核 |
| 项目总览 | 新建 / 打开项目、快捷运行各阶段 |
| 剧本 / 解析 / 资产 / 分镜 / 视频 | 分阶段操作、筛选、重生成、预览 |
| 设置 | **Image2 / Omni·Veo / Grok / 自定义** 的 Base URL、模型名、API Key；能力路由 |
| 演练模式 | 顶栏勾选 Dry-run：只组装 prompt，不调 API |

密钥保存在本机 `~/.config/adrama/settings.json`（Windows: `%APPDATA%\adrama\settings.json`），不写进项目目录。

### API 密钥：官方 / 自定义（每种独立）

| 厂商 | 官方 Key | 自定义 Key | 官方 Base URL |
|------|----------|------------|---------------|
| OpenAI / Image2 | `OPENAI_API_KEY` | `ADRAMA_OPENAI_CUSTOM_KEY` | `https://api.openai.com/v1` |
| Google / Veo | `GEMINI_API_KEY` | `ADRAMA_GOOGLE_CUSTOM_KEY` | Gemini `v1beta` |
| xAI / Grok | `XAI_API_KEY` | `ADRAMA_XAI_CUSTOM_KEY` | `https://api.x.ai/v1` |

在 GUI **设置 → API 密钥** 中为每家切换「官方 / 自定义」，自定义模式可填代理 URL；两套密钥互不影响。

**能力路由**（设置 → 能力路由）：对话 / 图像 / 视频 可分别指定使用 OpenAI、Google 或 Grok。

### 其它 GUI 能力

- 深色主题侧栏导航、卡片式页面、工作流画布
- 中文字体自动加载 · 取消任务 · 剧本编辑保存
- 图片预览 · 最近项目 · 连接测试 · API 自动重试

## 命令行

```bash
export OPENAI_API_KEY=...
export GEMINI_API_KEY=...

adrama new my-drama --style "cinematic, photorealistic" --aspect 16:9
cd my-drama
adrama import ../examples/mini_script.md

adrama parse --dry-run
adrama parse
adrama approve parse

adrama assets --dry-run
adrama assets
adrama approve assets

adrama storyboard
adrama approve storyboard

adrama video --shot shot_s01_01
adrama video --concat
adrama approve video

adrama status
```

### 命令一览

| 命令 | 说明 |
|------|------|
| `adrama` / `adrama gui` | 打开图形界面 |
| `adrama new <name>` | 初始化项目目录 |
| `adrama import <file>` | 导入剧本 |
| `adrama parse` | 阶段1：LLM 解析剧本 |
| `adrama assets [--only NAME]` | 阶段2：角色/服化道/场景图 |
| `adrama storyboard [--scene N] [--shot ID]` | 阶段3：分镜图 |
| `adrama video [--shot ID] [--concat]` | 阶段4：视频片段 |
| `adrama status` | 查看状态 |
| `adrama approve <stage>` | 审核通过，解锁下一阶段 |
| `adrama redo <stage> --id X` | 重生成单项 |

全局参数：`--project <dir>`、`--gui`、各生成命令的 `--dry-run`。

## 配置

`project.toml` 可覆盖模型 ID 与 API base URL：

```toml
name = "my-drama"
style = "cinematic, photorealistic, film grain"
aspect = "16:9"
openai_image_model = "gpt-image-1"
openai_chat_model = "gpt-4.1"
google_video_model = "veo-3.1-generate-preview"
openai_base_url = "https://api.openai.com/v1"
google_base_url = "https://generativelanguage.googleapis.com/v1beta"
```

也可在 GUI → Settings 中编辑并保存。模型名称以官方文档为准。
