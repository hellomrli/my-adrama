# adrama

**AI 短剧生产工作流：剧本 → 拆解 → 资产 → 分镜 → 视频。**
一套引擎，两个前端（桌面界面 / 命令行），Windows 与 Linux 都能跑。

每个阶段的产物都落盘为可读文件，**人工审核通过后才解锁下一阶段**——视频生成很贵，
这条门控是有意为之。任何一条资产 / 分镜 / 片段都能单独改 prompt、单独重生成。

---

## 下载与试用

云端构建产物在 [Releases](https://github.com/hellomrli/my-adrama/releases/latest)：

| 平台 | 文件 | 用法 |
|------|------|------|
| Windows | `adrama.exe` 或 `adrama-windows-x86_64.zip` | 双击 exe 直接开界面 |
| Debian / Ubuntu | `adrama_*_amd64.deb` | `sudo dpkg -i adrama_*_amd64.deb`，缺依赖时 `sudo apt-get install -f` |
| 其它 Linux | `adrama-linux-x86_64` | `chmod +x adrama-linux-x86_64 && ./adrama-linux-x86_64` |

Linux 需要一款中文字体（`sudo apt install fonts-noto-cjk`），否则界面显示为方块。
拼接成片需要系统里有 `ffmpeg`（可选）。

### 五分钟上手

1. 启动程序（无参数即打开界面）。
2. **设置 → 模型与密钥**：对「对话 / 生图 / 视频」三种能力分别选供应商、填地址与密钥，
   点「测试并拉取模型」，再从下拉框里选模型，最后「保存密钥」+「保存到项目」。
   模型列表是从上游实时拉的，官方上新或改名后重新点一次即可，不需要等程序更新。
3. **概览 → 新建项目**，选好画幅（横屏 16:9 / 竖屏 9:16）。
4. **剧本**页：导入 `.md/.txt` 或直接写，保存。
5. 回到**概览**，按流水线卡片逐阶段「运行 → 检查 → 审核通过」。
   - 先勾选顶栏 **演练模式** 跑一遍：只显示将要发送的 prompt，不花钱。
   - 每个阶段的页面里可以单条查看、改 prompt、重生成。
6. 视频阶段完成后，「拼接成片」得到 `video/final.mp4`。

> 建议第一次只跑一个镜头（视频页选中某条 → 重新生成此条）确认额度和效果，再批量。

---

## 界面

| 页面 | 内容 |
|------|------|
| **概览** | 四阶段流水线卡片（状态 / 完成度 / 运行 / 审核）、运行前检查（剧本、能力路由、密钥是否齐备）、拆解提醒 |
| **剧本** | 导入或直接编写剧本原文 |
| **流程图** | 拆解之后自动画出这个项目的**真实依赖图**：哪些资产会被哪个分镜当参考、哪个分镜生成哪个片段、最后拼成成片。节点颜色即状态，点节点直接跳到对应条目 |
| **拆解** | 结构化查看角色表与按场次分组的镜头表（景别、时长、台词），可切原始 JSON |
| **资产 / 分镜 / 视频** | 条目工作台：网格列出**全部应生成项**（含尚未生成的），右侧检查器可看大图、改 prompt、单条重生成；工具栏有「生成缺失 / 全部重生成 / 重试失败 / 审核通过」 |
| **设置** | **按能力配置且互相独立**：对话模型 / 生图模型 / 视频模型各一张卡——选供应商 → 官方或自定义地址 → 密钥 → 从拉取到的模型里选。三格的服务商、地址、密钥、模型都分开存 |

- 顶栏 **演练模式**：只组装并展示 prompt，不调用任何付费接口。
- 底部**控制台**：实时进度、逐条状态与失败原因；任务运行中随时可取消（视频轮询中也能立刻停）。
- 密钥存在 `~/.config/adrama/settings.json`（Windows：`%APPDATA%\adrama\settings.json`），
  权限 600，不会写进项目目录。

---

## 命令行

界面能做的，命令行都能做（同一套引擎）：

```bash
adrama new my-drama --style "cinematic, photorealistic" --aspect 16:9
cd my-drama
adrama import ../examples/mini_script.md

adrama parse --dry-run        # 只打印将要发送的 prompt
adrama parse
adrama approve parse

adrama assets                 # 缺什么补什么
adrama assets --only char_li_ming --force
adrama approve assets

adrama storyboard --scene 1
adrama approve storyboard

adrama video --shot shot_s01_01
adrama export                 # ffmpeg 拼接成片
adrama status
```

| 命令 | 说明 |
|------|------|
| `adrama` / `adrama gui` | 打开图形界面 |
| `adrama new <name>` | 初始化项目（`--style` / `--aspect`） |
| `adrama import <file>` | 导入剧本 |
| `adrama parse` | 阶段 1：拆解剧本为结构化 JSON |
| `adrama assets [--only ID]…` | 阶段 2：角色 / 服化道 / 场景图 |
| `adrama storyboard [--scene N] [--shot ID]…` | 阶段 3：分镜图 |
| `adrama video [--shot ID]…` | 阶段 4：视频片段 |
| `adrama export` | 拼接成片 |
| `adrama status` | 项目状态与各阶段完成度 |
| `adrama providers` | 查看能力路由与密钥配置情况 |
| `adrama test <capability>` | 测试该能力所配端点（`chat` / `image` / `video`） |
| `adrama models <capability>` | 列出该能力所配端点提供的模型（`--all` 看全部） |
| `adrama update [--apply]` | 检查新版本；加 `--apply` 直接下载安装 |
| `adrama approve <stage>` / `reset <stage>` | 审核通过 / 撤销审核 |

通用参数：`--project <dir>`、`--dry-run`、`--force`（覆盖已有产物）、
`--reset-prompt`（丢弃手改的 prompt，按 breakdown 重新组装）。

---

## 服务商与能力路由

「对话 / 图像 / 视频」三种能力各自指定由哪家承担：

| 服务商 | 对话 | 图像 | 视频 | 官方 Base URL |
|--------|------|------|------|---------------|
| OpenAI | ✓ | ✓ | — | `https://api.openai.com/v1` |
| Google | ✓ | ✓ | ✓ | `https://generativelanguage.googleapis.com/v1beta` |
| xAI / Grok | ✓ | ✓ | — | `https://api.x.ai/v1` |

把某种能力指到不支持它的服务商，会在开跑前直接报错并列出可选项，
而不是发出一个形状错误的请求让你去猜 400 的含义。

每家都有**官方 / 自定义**两套独立密钥与地址（自定义用于代理或自建网关），
切换模式不会覆盖另一套。环境变量可用于补空（不覆盖已保存的值）：

| 服务商 | 官方 Key | 自定义 Key |
|--------|----------|------------|
| OpenAI | `OPENAI_API_KEY` | `ADRAMA_OPENAI_CUSTOM_KEY` |
| Google | `GEMINI_API_KEY` / `GOOGLE_API_KEY` | `ADRAMA_GOOGLE_CUSTOM_KEY` |
| xAI | `XAI_API_KEY` / `GROK_API_KEY` | `ADRAMA_XAI_CUSTOM_KEY` |

设置页按能力组织，且**三格完全独立**：一张卡回答「这件事找谁做、连哪里、用什么密钥、跑哪个模型」。
即使对话和生图都选 OpenAI，也可以一个走官方、一个走你自己的中转，密钥与模型列表各存一份——
改一处不会动到另一处。同一个端点在别处填过密钥时，会出现「沿用…的密钥」按钮做一次性复制。

**模型不写死**：点「测试并拉取模型」会调用上游的 `/models`，把当前真实可用的模型列进下拉框
（相关的排在前面，其余的也都列出来），选中即写入 `project.toml`；列表会缓存下来，重启仍在。
代理若没实现 `/models`，手填也照常工作。

模型 ID 全部是配置项，官方改名时改配置即可。Google 的图像能力按模型名自动选端点：
`imagen-*` 走 `:predict`，`gemini-*` 图像模型走 `:generateContent`——只有后者支持参考图，
**角色一致性**依赖它。

---

## 在线更新

**设置 → 关于与更新**：显示当前版本与安装方式，可手动「检查更新」，也可勾选启动时自动检查
（每天最多问一次 GitHub，失败不打扰）。发现新版本时顶栏会出现「有新版本」徽标。

- **绿色版**（下载的 `adrama.exe` / `adrama-linux-x86_64`）：可以直接「下载并安装」，
  下载完会用 release 附带的 `SHA256SUMS.txt` 校验，通过后原地替换，点「立即重启」即可用上新版。
- **.deb 安装的副本**：程序不会去动 `/usr/bin` 里的文件（那是包管理器的地盘），
  会提示你用 apt / dpkg 更新，并给出发布页链接。

命令行同理：`adrama update` 查看，`adrama update --apply` 直接更新。

## 项目目录（即持久化状态）

```
my-drama/
├── project.toml          # 风格、画幅、能力路由、各服务商端点与模型、生成参数
├── state.json            # 各阶段审核状态
├── script/               # 剧本原文
├── parsed/breakdown.json # 角色 / 场景 / 场次 / 镜头
├── assets/…              # 每个资产一个目录：图片 + prompt.txt + meta.json
├── storyboard/…          # <shot>.png + <shot>.json
└── video/…               # <shot>.mp4 + <shot>.json（含任务 id，中断后可续查）
```

全是可读文件：可以手改 prompt、替换某张图，再继续跑。
`prompt.txt` 与 sidecar 里的 `prompt` 只要非空，就是下次生成使用的文本；
想回到自动组装的版本用「恢复默认」或 `--reset-prompt`。

`project.toml` 示例：

```toml
name = "my-drama"
style = "cinematic, photorealistic, film grain"
aspect = "16:9"

[generation]
image_concurrency = 3     # 图像并发
video_concurrency = 1     # 视频串行，避免成本失控
max_shot_seconds = 8
video_poll_secs = 10
video_timeout_secs = 1800
request_retries = 3

# 三种能力各自一段，互不影响
[endpoints.chat]
provider = "openai"
mode = "official"          # official | custom
model = "gpt-4.1"

[endpoints.image]
provider = "openai"
mode = "custom"            # 同一家，但走自己的中转
custom_base_url = "https://image-relay.example.com/v1"
model = "gpt-image-1"

[endpoints.video]
provider = "google"
mode = "official"
model = "veo-3.1-generate-preview"
```

0.1.x / 0.2.x 的旧 `project.toml`（平铺字段，或 `routing` + `providers`）
打开时自动迁移成上面的格式，老项目直接继续用；本机保存的密钥同样会自动拆到三种能力下。

---

## 0.3.3 有什么变化

- 演练模式下不再把整个剧本糊满控制台：控制台只显示摘要（发往哪个端点、多少字、开头几行），
  完整 prompt 写进日志文件；并明确提示「这只是演练，要真跑请关掉演练模式」。
- 自检说清楚它验证的是什么，以及真正开跑该点哪里（左侧「解析」→「运行拆解」）。

## 0.3.2 有什么变化

- **自检面板**（设置 → 关于与更新 → 自检）：一屏列出版本、安装方式、演练模式、
  后台是否空闲、项目与剧本、三种能力的服务商/地址/模型/密钥状态；
  可「运行自检（演练，不花钱）」验证整条链路，也可一键复制诊断信息。
- **日志写文件**：图形界面下 stderr 往往没人看得到，现在统一写到
  `~/.config/adrama/adrama.log`（Windows：`%APPDATA%\adrama\adrama.log`），
  自检面板里可直接打开。
- **不会再默默卡住**：任务提交 3 秒还没开始就会在控制台明说，并提示去看自检。

## 0.3.1 有什么变化

- **修掉「点了运行却毫无反应」**。启动时的更新检查和任务跑在同一个后台线程上，
  GitHub 连不上时（国内常见）那个检查会一直挂到超时，这期间点「运行」，
  请求只是进了队列，界面和上游都没有任何动静。现在杂务有独立线程，永远不挡任务；
  更新检查也加了 8 秒连接超时。
- **点击必有反馈**：提交任务会立刻在控制台留一行「已提交：…」，后台线程若已停止会直接报错。
- **界面里改完不用先记得保存**：运行前会自动把未保存的密钥与项目配置落盘——
  以前改了服务商/模型却没点保存，跑的还是磁盘上的旧配置。
- **演练模式不再容易忘**：开启时顶栏有常驻警示徽标（可点击关闭），
  运行按钮也会写成「运行拆解（演练）」。
- 后台任务若发生内部错误不再让界面永远停在「运行中」。

## 0.3.0 有什么变化

- **对话 / 生图 / 视频三种能力彻底独立**。以前服务商、端点地址和密钥是按「服务商」存的，
  两种能力选了同一家就会共用，改一处影响另一处。现在每种能力自己存一份
  服务商 + 地址 + 密钥 + 模型，互不干扰——同是 OpenAI，对话走官方、生图走你的中转，
  这是常见需求。
- `project.toml` 改为 `[endpoints.chat|image|video]` 三段；旧格式（0.1.x 平铺字段、
  0.2.x 的 `routing` + `providers`）打开时自动迁移，本机密钥也会自动拆到三种能力下。
- 缺密钥的报错会点名是哪种能力，而不是笼统地说某家服务商。
- CLI：`adrama test <chat|image|video>`、`adrama models <chat|image|video> [--all]`。

## 0.2.3 有什么变化

- **拆解请求改为流式**。长剧本 + 慢模型经常要一两分钟，非流式请求在
  Cloudflare 之类的网关上会被判成 **HTTP 524（网关超时）**。流式下字节持续到达，
  连接不空闲，就不会被掐断。OpenAI 兼容与 Gemini 两条路都改了；
  上游若不支持流式，会自动按整包响应解析。
- **降级不再被误触发**。以前只要拆解请求出错就换 `json_object` 兼容模式，
  于是一次超时会变成「再花一次钱、还拿到更差的结果」。现在只有上游明确拒绝
  请求体（400/404/422 等）或返回的内容不是合法 JSON 时才降级。
- **Cloudflare 的 52x 纳入重试**，并给出人话解释（524 会直接告诉你是网关超时、
  以及可以换更快的模型 / 缩短剧本 / 改用官方端点）。

## 0.2.2 有什么变化

- 设置页的密钥部分改为**按能力组织**：对话模型 / 生图模型 / 视频模型各一张卡，
  先选供应商，再填地址与密钥，最后挑模型；原来的「能力路由」标签页并入其中。
- 卡片右上角直接标出这项能力是否「可用」；两种能力共用同一家时会提示。
- 新增「已保存的密钥」一览，可查看和清除任意一套密钥。

## 0.2.1 有什么变化

- **在线更新**：应用内检查 / 下载 / 校验 / 原地替换，见上一节。
- **模型不再写死**：测试密钥时顺带拉取上游模型列表，在下拉框里选，官方上新即可用。
- **流程图**：拆解后自动画出本项目的真实依赖图，可点节点跳转。

## 0.2.0 有什么变化

整体重构 + 界面重写。值得注意的行为变化：

- **能力路由现在是真的**。0.1.x 无论怎么配都只会调 OpenAI（图像/对话）或 Google（视频），
  路由到别家会发出形状错误的请求；现在按能力构造对应客户端，不支持的组合直接拒绝。
- **审核门控真的检查产物**。旧版只看输出目录在不在（而目录是新建项目时就建好的），
  空分镜也能 approve 并解锁最贵的视频阶段。
- **进度与取消可用**。逐条状态、进度条、失败原因都会实时出现在控制台；
  取消能打断视频轮询，而不是等 30 分钟。
- **条目级审校**：每条产物都能单独查看、改 prompt、重生成；未生成的条目也会列出来。
- **手改 prompt 现在生效**（旧版写了 `prompt.txt` 但从不读取）。
- **密钥不再写进程环境变量**，改为随任务传值；配置文件权限收紧到 600。
- 图像可并发（默认 3）、视频默认串行；重试基于 HTTP 状态码而不是匹配错误文本。
- 视频任务 id 先落盘再等待，中断/超时后重跑会**续查**而不是重新付费。

---

## 从源码构建

```bash
cargo build --release      # 或 cargo install --path .
cargo test                 # 68 个单元测试，不需要网络
```

依赖：Rust 1.75+。Linux 构建 GUI 需要
`libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libgtk-3-dev pkg-config`。

从 Linux 交叉编译 Windows 包（需 [llvm-mingw](https://github.com/mstorsjo/llvm-mingw)）：

```bash
rustup target add x86_64-pc-windows-gnullvm
./scripts/package-windows.sh     # 产物在 dist/
```

架构说明见 [DESIGN.md](DESIGN.md)。许可证 MIT。
