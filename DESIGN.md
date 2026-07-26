# adrama 架构说明

> 目标：一条「剧本 → 拆解 → 资产 → 分镜 → 视频」的 AI 短剧生产流水线，
> 每个阶段产物落盘、可人工审核、可单条重生成。视频生成很贵，所以**门控**与
> **单条粒度重跑**是架构的第一约束，而不是事后补的功能。

## 分层

依赖只向下，不回指：

```
ui/  (egui 桌面端)        cli.rs (命令行)
        └───────┬────────────┘
                ▼
             engine/            阶段编排、门控、任务分发、事件流
                ▼
           providers/           能力 trait + HTTP 客户端（OpenAI 兼容 / Gemini）
                ▼
             model/             纯数据 + 磁盘布局（无 HTTP、无密钥、无 UI）
```

| 模块 | 职责 | 不做什么 |
|------|------|----------|
| `model/` | `project.toml`、`state.json`、`breakdown.json` 的结构与读写；产物索引 | 不发请求、不读环境变量 |
| `providers/` | `ChatProvider` / `ImageProvider` / `VideoProvider` 三个能力 trait 及其实现；密钥以值传递 | 不知道「阶段」的存在 |
| `engine/` | 四个阶段的编排、prompt 组装、门控规则、`Job` 分发、事件与取消 | 不知道前端是谁 |
| `cli.rs` / `ui/` | 把 `Job` 交给 engine，把事件渲染成终端输出或界面 | 不含业务规则 |

## 关键设计

### 1. 能力是配置的第一维度

项目配置里，三种能力各自声明「谁来做、连哪里、用哪个模型」：

```toml
[endpoints.chat]
provider = "openai"
mode = "official"
model = "gpt-4.1"

[endpoints.image]
provider = "openai"      # 同一家……
mode = "custom"          # ……但走自己的中转，与对话互不影响
custom_base_url = "https://image-relay.example.com/v1"
model = "gpt-image-1"
```

**不共享**是刻意的。早期版本把端点和密钥按服务商存，两种能力选同一家就会共用，
用户改对话的中转地址、生图跟着变——这是个真实踩到的坑。密钥同样按
「能力 + 服务商 + 模式」存放，界面提供一次性复制按钮，而不是隐式共享。

`ProviderFactory` 按能力构造对应客户端，并在构造时就拒绝不可能的组合
（例如把「视频」路由到只有对话/图像能力的服务商），而不是发出一个形状错误的
HTTP 请求再让用户去猜 400 的含义。新增服务商 = 实现 trait + 在 `ProviderId`
增加一个分支，不需要改阶段代码。

### 2. 旧配置一律自动迁移

`RawConfig` 同时接受三种形态：当前的 `[endpoints.*]`、0.2.x 的
`routing` + `providers`、以及 0.1.x 的平铺 `openai_chat_model` 字段，
统一转换成按能力分组的结构。用户升级时不需要手工改任何文件。

### 3. 密钥是值，不是进程环境

旧版把密钥 `set_var` 进进程环境，UI 线程写、worker 线程读——既是数据竞争，
也没法按任务隔离。现在密钥装在 `Credentials` 里随 `JobRequest` 传递；环境变量
只在启动时**读取**一次用于补空。密钥文件写在用户配置目录且 `chmod 600`。

### 4. 阶段产出事件，而不是 println

`StageEvent`：`Started / Log / Progress / Item / Artifact / Finished`。
CLI 渲染成 indicatif 进度条，GUI 渲染成进度条 + 控制台 + 单条状态。
取消令牌 `CancelToken` 一路传到条目循环内部和 Veo 轮询循环内部——
30 分钟的视频等待期间点「取消」是真的会停。

### 5. 门控看产物，不看目录

`approve` 之前会真的去数产物（breakdown 里有没有镜头、有没有图、有没有 mp4）。
旧实现只检查目录是否存在，而目录是 `new` 时就建好的，等于形同虚设。
撤销某阶段的审核会连带撤销其下游阶段的审核。

### 6. Prompt 落盘即权威

`assets/<kind>/<id>/prompt.txt` 与分镜/视频的 `<shot>.json` 里的 `prompt`
字段，只要非空就是下次生成使用的文本。界面里改完保存，或者用编辑器手改，
都直接生效；「恢复默认」清空它即可回到自动组装的版本。

### 7. 产物索引是派生的

`ProjectIndex` 由 breakdown（应该有什么）与磁盘（实际有什么）合成，
所以未生成的条目也会以「待生成」出现在列表里，而不是等生成后才凭空冒出。
状态以磁盘为准：sidecar 说 done 但文件不在，就是待生成。

### 8. 更新与模型都不写死

模型列表由 `/models` 实时拉取后缓存在用户配置里，UI 只做「相关的排前面」的排序，
不做过滤——厂商改名太频繁，隐藏比列出更危险。

在线更新只信任两件事：到 github.com 的 HTTPS，以及 release 附带的 `SHA256SUMS.txt`。
下载地址必须落在 GitHub 域内，校验不过就整体放弃。包管理器装的副本一律不自我替换。
Windows 上运行中的 exe 不能删除但可以改名，于是先把自己改成 `.old` 再放新文件，
下次启动清理；Linux/macOS 直接 `rename` 覆盖，正在运行的进程持有旧 inode 不受影响。

## 目录结构（即持久化状态）

```
my-drama/
├── project.toml          # 名称、风格、画幅、routing、providers、generation
├── state.json            # 四个阶段的审核状态
├── script/               # 剧本原文
├── parsed/breakdown.json # 角色 / 场景 / 场次 / 镜头
├── assets/
│   ├── characters/<id>/{front,side,full}.png + prompt.txt + meta.json
│   └── costumes|props|locations/<id>/ref.png + prompt.txt + meta.json
├── storyboard/<shot>.png + <shot>.json
└── video/<shot>.mp4 + <shot>.json（含 operation id，可断点续查）
```

全部是可读文件：用户可以手改 prompt、换掉某张图，再继续跑。

## 阶段要点

| 阶段 | 关键点 |
|------|--------|
| 拆解 | **流式**请求（避免网关 100 秒超时判 524）。OpenAI 走 `json_schema`，仅在上游明确拒绝请求体或返回非法 JSON 时降级到 `json_object`——超时不降级，否则等于再烧一次钱；Gemini 的 `responseSchema` 不接受 `additionalProperties` 与联合类型，由 `to_gemini_schema` 翻译 |
| 资产 | 角色三视图（正/侧/全身），同一段身份描述复用；服化道与场景各一张参考图 |
| 分镜 | **一致性最大的风险点**：把角色定妆照 + 场景图作为参考图输入，同时把外貌描述冗余写进 prompt。图像模型若不支持参考图，会明确告警而不是静默丢失 |
| 视频 | 提交后先把 operation id 落盘再等待，中断后重跑会**续查**而不是重新付费；超时同样保留 id |
| 成片 | ffmpeg concat；列表用相对文件名，避免中文/空格路径把命令搞挂 |

## 前端的两条线程

任务线程跑 `engine::run_job`；**杂务线程**（更新检查/下载）完全独立。
两者曾经合并过一次，结果是：GitHub 连不上时更新检查挂住线程，用户点「运行」
只会静静排队，界面和上游都毫无动静。教训是——**任何可能挂住的网络调用，
都不能和用户触发的任务共用一条队列**。`runtime.rs` 里有对应的回归测试。

任务提交后界面立刻写一行「已提交」，即使后台正忙，用户也能确认自己点到了。

## 并发与成本

`[generation]` 段控制：图像默认 3 并发、视频默认串行、单镜头时长上限、
轮询间隔、超时、重试次数。重试只针对 429/5xx 这类状态码，而不是靠匹配
错误文本里的关键词。

## 测试

`cargo test` 覆盖：配置迁移与序列化往返、能力路由与缺密钥报错、阶段门控与
审核撤销、prompt 组装（含参考图冗余与场次继承）、产物索引的磁盘优先规则、
并发执行器的计数与取消、密钥脱敏。不需要网络。
