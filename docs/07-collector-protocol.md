# 采集器协议与内置采集器

> 本仓库的采集器 bin（`viacli-hazards` / `viacli-markets` / `viacli-space` /
> `viacli-rss` / `viacli-sys` / `viacli-networkdevice`）都实现下面这份协议：从 stdin 读一行 JSON 请求、
> 向 stdout 写 NDJSON 消息、`--describe` 输出自描述。因此它们既可以被本仓库直接运行，
> 也可以作为第三方采集器挂到任何兼容的宿主（如 keendeck）下。
>
> 快速验证：
>
> ```bash
> viacli-hazards --describe | jq .
> echo '{"protocol":1,"runId":"t","now":0,"mode":"oneshot","config":{}}' | viacli-hazards | jq -c .
> ```
>
> | bin | 采集内容 | 依赖的外部源 |
> |---|---|---|
> | `viacli-hazards` | 地震 / 天气预警 / 空间天气 | USGS、NWS、NOAA SWPC |
> | `viacli-markets` | 预测市场行情 | Polymarket、Kalshi |
> | `viacli-space` | 卫星星下点 / 地面站 | CelesTrak、SatNOGS（TinyGS 默认关） |
> | `viacli-rss` | RSS/Atom 条目 | 内置中文源 + 自定义订阅 |
> | `viacli-sys` | 本机 CPU/内存/磁盘/网络/温度/进程 | 本机 |
> | `viacli-networkdevice` | 局域网设备发现 / 端口 / 厂商（自带 SQLite 清点库） | 本机 + `ip`（可选 `nmap`） |
>

> 本文档是**独立标准**，定义"采集器（Collector）如何与宿主（Host）通讯"。
> 它不依赖任何具体实现或其它设计文档，可以单独交给第三方采集器作者使用。

状态：草案。规范性用词 **MUST / MUST NOT / SHOULD / MAY** 按 RFC 2119 解释。

---

## 1. 术语与角色

| 术语 | 含义 |
| --- | --- |
| **Host（宿主）** | 调度采集、提供上下文、接收数据并落库的一方（本项目的 core）。Host 是**唯一写入者**。 |
| **Collector（采集器）** | 产出数据的一方。可以是：**(a) 宿主进程内的 Rust 模块**（内置采集器）；**(b) 独立可执行程序 / 脚本**（第三方采集器）。 |
| **Run（一次运行）** | Host 启动 Collector 到其退出的完整过程。 |
| **Message（消息）** | 通讯的最小单元，一个 JSON 对象。 |
| **Series（序列）** | 一条数值时间线：一个名字 + 元数据（单位/类型）。 |
| **Sample（样本点）** | 序列上的一个点 `(series, ts, value)`。 |
| **Record（记录）** | 一条 JSON 快照 `(kind, key, ts, payload)`。 |

**核心原则：Collector 只产出数据，绝不写库。** 存储语义（幂等、保留、聚合）由 Host 统一负责。
这条原则是三种收益的前提：内置与第三方行为一致、第三方不需要数据库权限、采集器可以脱离存储单独测试。

### 1.1 本协议规定什么、不规定什么

**规定**（双方契约，任一 Host 实现 × 任一 Collector 实现都必须一致）：

* 消息模型与编码（§2）、序列命名（§2.4）、时间语义（§2.5）；
* 两种运行模式各自**是什么**：`oneshot` = Host 调用一次、采集器干完就退；
  `stream` = Host 保持它运行、持续收货（§4）；
* 退出码与错误类别（§3.4、§6）、状态（§5）、自描述与能力声明（§8）。

**不规定**（Host 的内部机制，属于实现自由）：

* Host **怎么**按时调用 `oneshot`（定时器、串行、错过是否补跑）；
* Host **怎么**保活、监视、重启 `stream`，退避策略、健康如何判定；
* 并发上限、资源配额的具体数值、UI 怎么展示。

> 理由：把"一种 Host 的实现细节"写进协议，等于要求所有 Host 都这么实现，
> 而第三方采集器作者并不需要知道这些。**协议越薄，越容易被多种实现正确实现。**
> 本仓库把这些机制写在 `DESIGN.md` §6.2.13。

---

## 2. 消息模型（规范性核心）

### 2.1 编码与分帧

* 消息编码为 **JSON**，**每行一个对象**（NDJSON / JSON Lines）。
* 编码 **MUST** 为 UTF-8。
* 行分隔符：发送方 **SHOULD** 使用 `LF`（`\n`）；接收方 **MUST** 容忍行尾的 `CR`（`\r\n`）。
* 空行 **MUST** 被忽略。
* **每行 MUST 是一个完整的 JSON 对象**，不得跨行。单行 **SHOULD NOT** 超过 8 MiB；
  更大的数据 **MUST** 拆成多条消息（例如一条记录一行）。
* 发送方 **MUST** 及时 flush（见 §3.3），不得依赖进程退出时刷新。

### 2.2 请求（Host → Collector）

Host 在启动 Collector 后，向其 **stdin 写入一行 JSON** 作为请求，然后**关闭写端**（oneshot 模式）
或保持打开（stream 模式，见 §4.2）。

```json
{
  "protocol": 1,
  "runId": "0f3c1a2e-…",
  "collector": "system",
  "datasource": "local-1",
  "now": 1799999999123,
  "mode": "oneshot",
  "intervalMs": 2000,
  "config": { "categories": ["cpu", "memory"], "topProcesses": 5 },
  "state": { "…": "…" },
  "limits": { "timeoutMs": 30000, "maxOutputBytes": 67108864, "maxLines": 1000000 },
  "cwd": "/path/to/collector"
}
```

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `protocol` | ✅ | 协议版本，整数。当前为 `1`。 |
| `runId` | ✅ | 本次运行的唯一标识，用于日志关联。 |
| `now` | ✅ | Host 的当前时间（Unix 毫秒）。Collector **SHOULD** 用它作为缺省 `ts`（见 §2.5）。 |
| `mode` | ✅ | `oneshot` \| `stream`（§4）。 |
| `intervalMs` | stream 模式必填 | 期望的产出间隔（毫秒）。 |
| `config` | ✅ | 用户配置（对象）。结构由采集器自己的 `configSchema` 定义。 |
| `state` | ✕ | 上次成功运行保存的状态（§5）。首次运行为 `null` 或缺失。 |
| `limits` | ✕ | Host 施加的限制，Collector **SHOULD** 遵守（Host 也会强制）。 |
| `collector` / `datasource` / `cwd` | ✕ | 身份与工作目录，仅供日志与相对路径解析。 |

**替代配置来源**：Collector **MAY** 支持 `--config <path>`（读取同一结构的 JSON 文件）。
若同时提供 stdin 与 `--config`，**stdin 优先**。

### 2.3 响应消息（Collector → Host，stdout）

每条消息 **MUST** 有 `type` 字段。共 7 种：

| `type` | 用途 | 必需性 |
| --- | --- | --- |
| `meta` | 采集器自述：名称、版本、能力 | MAY（首个消息） |
| `series` | 声明序列的元数据（名字/单位/类型） | SHOULD（在首个 `sample` 之前） |
| `sample` | 一个样本点 | — |
| `record` | 一条 JSON 记录 | — |
| `state` | 提交本次运行结束后的状态 | MAY |
| `log` | 结构化日志 | MAY |
| `error` | 显式错误 | MAY（或改用退出码） |

**未知 `type` 或非法 JSON 行 → Host MUST 视为协议错误并中止本次运行，且在错误信息中附上原始行。**
（静默跳过是禁止的：那会让人误判为"采集器没有产出"，问题极难定位。）

#### `series`（序列声明）

```json
{"type":"series","name":"system/cpu_usage","unit":"percent","kind":"gauge",
 "help":"总体 CPU 使用率","labels":{"host":"web-01"}}
```

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `name` | ✅ | 序列名，规则见 §2.4。**全局唯一标识**。 |
| `kind` | ✅ | `gauge` \| `counter`，语义见 §2.6。 |
| `unit` | ✕ | 单位，推荐取值见 §2.7。缺省视为 `count`（无量纲）。 |
| `help` | ✕ | 人类可读描述，Host 可用于 UI 提示。 |
| `labels` | ✕ | 附加维度（对象，值为字符串）。**不参与身份**，仅作展示/过滤。 |
| `persist` | ✕ | 是否落库。缺省视为 `true`。`false` = **只为实时展示**，不要存（§2.8）。 |
| `maxRetentionMs` | ✕ | **保留上限**（毫秒）。缺省 = 无意见；`null` = 永久（§2.8）。 |

同一 `name` **MAY** 被多次声明；**MUST** 语义一致（Host **SHOULD** 以首个声明为准并记录不一致）。

**同一个 `name` 的含义 MUST 与运行模式无关。** 若某个量在 `oneshot` 与 `stream` 下
算出来的东西不是一回事（例如"瞬间快照"与"整段平均"），采集器 **MUST NOT** 用同一个名字发它——
要么换名字，要么只在能保证语义一致的模式下产出。宿主会把两种模式的数据存进同一条序列，
**同名不同义会静默地污染历史**，事后无法分辨。

> **为什么要有 `series` 消息**：单位、类型、描述这些元数据对展示是必需的
> （"这个数字是字节还是百分比？"），但每个样本点重复携带它们既浪费又容易不一致。
> 声明一次、样本点只带名字，是 Prometheus 等系统的成熟做法。

#### `sample`

```json
{"type":"sample","name":"system/cpu_usage","ts":1799999999123,"value":12.5}
```

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `name` | ✅ | 序列名。**若此前未声明过，Host SHOULD 以缺省元数据（`kind: gauge`、`unit: count`）接受**，但这是不推荐的用法。 |
| `value` | ✅ | 数值：JSON 整数 → int64；带小数/指数 → float64。见 §2.6。 |
| `ts` | ✕ | Unix 毫秒。缺省用请求里的 `now`。 |

#### `record`

```json
{"type":"record","kind":"interface","key":"eth0","ts":1799999999123,
 "payload":{"state":"up","speedMbps":1000,"addrs":["10.0.0.5/24"]}}
```

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `kind` | ✅ | 记录类型（用于按类型查询与展示分组）。 |
| `key` | ✕ | 稳定标识。给出时同一轮内 `(kind,key,ts)` 去重；不给则每条独立。 |
| `payload` | ✅ | **JSON 对象**（不得二次编码为字符串）。 |
| `ts` | ✕ | 同上。 |

#### `state` / `log` / `error`

```json
{"type":"state","state":{"prevCpuTicks":123456}}
{"type":"log","level":"info","message":"sampled 8 interfaces"}
{"type":"error","kind":"upstream","retryable":true,"message":"permission denied reading /dev/nvidia0"}
```

* `log.level` ∈ `debug|info|warn|error`。
* `error.kind` ∈ `config|auth|network|upstream|parse|internal`（§6）。
* **一条 stream 消息只描述一次失败**；进程的最终成败由退出码决定（§6.2）。

### 2.4 序列命名规则（规范性）

```
name        = segment *( "/" segment )
segment     = 1*( ALPHA / DIGIT / "_" / "-" / "." )
```

* 总长度 **MUST** ≤ 255 字节（UTF-8 编码后）。
* 分隔符 `/` 表示层级，用于 UI 分组与权限前缀匹配。
* 首段的第一个字符 **MUST** 是字母。
* 每个段 **MUST NOT** 以 `-` 或 `.` 开头。
* 字符集限定为 `A-Za-z0-9_.-` 与 `/`；**不允许空格、中文、引号等**。

合法示例：

```
system/cpu_usage                       system/cpu_usage/core/0
system/memory_used_bytes               system/net/eth0/rx_bytes
system/disk/root/used_bytes            gpu/nvidia/0/temperature_celsius
host/web-01.prod/uptime_seconds
```

> **为什么允许 `_` 和 `.`**：指标命名惯例是 snake_case（`cpu_usage`、`http_requests_total`），
> 主机名/IP 惯例含点（`web-01.prod`、`10.0.0.5`）。若不允许，绝大多数真实指标名都得改写，
> 采集器作者会立刻撞墙。**不允许空格与中文**是为了让名字能安全地出现在 URL、
> 命令行与数据库键中；需要人类可读描述时用 `help` 字段。

### 2.5 时间语义

* `ts` 为 **Unix 毫秒（int64）**，表示**采样发生的时刻**。
* 省略 `ts` 时 Host **MUST** 使用请求中的 `now`。
* **Collector MUST NOT 自行"造"时间**（例如用进程启动时间推算）：同一轮内多个点的 `ts` 应当一致或按真实采样时刻递增，
  否则会破坏聚合与"取最新"。
* Host **SHOULD** 拒绝明显异常的时间（例如早于 2000 年或晚于当前时间 + 1 天）。

### 2.6 数值类型与 `kind` 语义

* `value` 为 JSON 整数 → **int64**（保持精确，不转浮点）；为小数/指数 → **float64**。
  > 注意 JavaScript 的 number 均为 double，`JSON.stringify(42.0)` 会输出 `42`——
  > JS 采集器无法表达"浮点 42"，但这不影响展示。
* `kind: gauge`：数值**本身有意义**（内存已用字节数、温度、当前使用率）。
* `kind: counter`：**单调递增**的累计量（网卡累计字节数、CPU 累计时间）。
  **消费方 MUST NOT 直接展示 counter，MUST 先做速率换算**（`rate`）。
* **可返回负值的 delta**（如磁盘可用空间变化）**MUST** 用 `gauge` 表达。

### 2.7 推荐单位

| `unit` | 含义 | 例子 |
| --- | --- | --- |
| `bytes` | 字节 | `system/memory_used_bytes` |
| `bytes_per_second` | 字节/秒 | `system/net/eth0/rx_rate` |
| `percent` | 百分比（0–100） | `system/cpu_usage` |
| `celsius` | 摄氏度 | `system/temperature/cpu_package` |
| `seconds` | 秒 | `system/uptime_seconds` |
| `hertz` | 赫兹 | `system/cpu_frequency` |
| `watts` / `volts` / `amperes` | 电学量 | `gpu/nvidia/0/power` |
| `rpm` | 转速 | `system/fan/cpu` |
| `count` | 无量纲计数（缺省） | `system/process_count` |

未列出的单位 **MAY** 使用，但 **SHOULD** 用上面的语义单位而不是自造（便于 UI 自动换算与格式化）。

---

### 2.8 保留声明：数据该留多久（`persist` / `maxRetentionMs`）

**为什么必须由采集器声明**：同一套保留策略不可能适用于所有数据——
3 秒一个点的 CPU 使用率放一年是垃圾，一篇抓回来的文章丢一天就是永久损失。
而"这条数据属于哪种"**只有采集器知道**，宿主无法从数值里推断。
宿主只知道 `unit` 和 `kind`，不知道"这是转瞬即逝的读数"还是"这是不可再生的档案"。

#### 两个字段，回答两个不同的问题

| 字段 | 回答 | 取值 |
| --- | --- | --- |
| `persist` | **需要持久化吗？** | `true`（缺省） / `false` |
| `maxRetentionMs` | **最多留多久还有价值？** | 缺省（无意见） / 正整数 / `null`（永久） |

四种组合的含义：

| `persist` | `maxRetentionMs` | 含义 | 典型数据 |
| --- | --- | --- | --- |
| 缺省 | 缺省 | **无意见**，用宿主策略 | 大多数指标 |
| `true` | 正整数 `N` | 超过 `N` 毫秒就没有保留价值 | 3 秒粒度的 CPU/速率读数：1 小时 |
| `true` | `null` | **永久保留**（不可再生） | 抓取回来的文章、审计记录、主机身份 |
| `false` | —（**MUST NOT** 出现） | **不落库**，只为实时展示 | 只在面板上滚动的实时信号 |

* `persist: false` 时 **MUST NOT** 同时给 `maxRetentionMs`（两者矛盾，宿主 MUST 视为协议错误）。
* `maxRetentionMs: 0` **不是**"不持久化"——要表达那个意思请用 `persist: false`。
  单独一个 `0` 在弱类型语言（如 JS）里是 **falsy**，会被前端误当成"没配"，
  用一个显式布尔值可以避开这个坑。

#### 语义：这是**提示**，不是担保

* 采集器声明的是"**这条数据在多久之后还有价值**"，不是"你必须留这么久"，
  也不是"你不许留更久"。
* **用户配置优先**：用户可以为了自己的理由（容量规划、审计）把保留期配得更长或更短；
  宿主 **MUST** 让用户配置覆盖采集器声明。
* 宿主在用户没有显式配置时，**SHOULD** 按 `min(全局默认, maxRetentionMs)` 取保留期——
  这样"3 秒粒度的瞬时读数"不会把 7 天的库撑满。
* `persist: false` 的数据，宿主 **MAY** 在内存里保留最近的值用于实时展示，
  但 **MUST NOT** 承诺提供历史（§7 的幂等与去重只对落库数据有意义）。

#### 记录类型：在 `--describe` 里声明

序列是**动态**的（有哪些网卡/挂载点取决于运行环境），所以随 `series` 消息声明；
记录类型是**静态**的（由采集器代码决定），所以放在 `--describe` 的 `recordKinds` 里（§8.1）。
两者用**同一套字段**。

> 记录类型此前没有任何声明渠道，这是一个既有空白——顺带在这里补上。
> 未列入 `recordKinds` 的记录类型仍然合法（例如类型由用户配置决定时），
> 此时宿主按数据模型使用自己的默认策略。

---

## 3. 传输与进程约定

### 3.1 传输 A：外部进程（规范性）

* Host 以**子进程**方式启动 Collector，**stdin 传请求、stdout 收消息、stderr 收日志**。
* **stdout 只放协议消息，一根头发都不能掺。** 任何调试输出 **MUST** 走 stderr。
  这是最常见的事故来源：`print("debug")` / `console.log(...)` 会污染数据流。
* Collector **MUST NOT** 在 stdout 输出非协议内容，包括启动横幅、进度条。
* 不经 shell 启动（Host **MUST** 用 `command` + `args` 数组），避免引号与注入问题。

### 3.2 传输 B：进程内 Rust 模块（规范性）

内置采集器不经过进程边界，但**消息模型完全一致**（§2）。宿主提供等价的 API：

```rust
// 概念示意：内置采集器的写入端与外部进程的 stdout 等价
fn collect(ctx: &Ctx, out: &mut impl MessageSink) -> Result<(), CollectError> {
    out.series("system/cpu_usage", Unit::Percent, Kind::Gauge, "总体 CPU 使用率");
    out.sample("system/cpu_usage", None, 12.5.into());
    Ok(())
}
```

**为什么坚持同一套消息模型**：存储层只需一条写入路径；内置与第三方的行为、元数据、
错误语义完全一致；协议演进时两边一起演进。

### 3.3 缓冲：必须及时 flush

* 当 stdout 不是终端（Host 用管道读取时就是这种情况），多数运行时会切换到**块缓冲**。
* Collector **MUST** 保证消息被及时写出，而不是等进程退出：
  * C/C++：`setvbuf(stdout, NULL, _IOLBF, 0)` 或每条消息后 `fflush`；
  * Python：`python -u`，或 `PYTHONUNBUFFERED=1`，或每条消息后 `flush=True`；
  * Node/JS：`process.stdout.write` 通常即时，但仍 **SHOULD** 避免整批拼接后再写。
* **不 flush 的后果**：Host 读不到任何消息，表现为"采集器卡住直到超时"，而采集器自己看起来一切正常。
  这是本协议**最容易踩且最难排查**的问题。

### 3.4 退出码

| 退出码 | 含义 | Host 处理 |
| --- | --- | --- |
| `0` | 成功 | 提交 `state`（若有），记录成功 |
| `2` | 配置错误（永久） | 不重试，提示用户修改配置 |
| `3` | 认证/凭据错误（永久） | 不重试，提示更新凭据 |
| `4` | 上游/依赖不可用（可重试） | 退避重试 |
| `5` | 采集器内部错误（可重试） | 退避重试 |
| 其它非零 | 未分类失败 | 按可重试处理，并附 stderr 尾部 |

* Host **SHOULD** 在退出码与 `error` 消息同时存在时，**以退出码为准**（消息作为补充说明）。
* Host **MUST** 保存 stderr 的**尾部**（如最后 4 KiB）用于报障。

---

## 4. 运行模型

采集器 **MUST** 支持 `--describe` 里 `modes` 声明的一种或两种模式。
模式决定**由谁驱动节奏**——这一点是本协议的全部内容：

| 模式 | 谁驱动节奏 | 采集器做什么 |
| --- | --- | --- |
| `oneshot` | **Host**：按时调用 | 被调用**一次**，产出、退出 |
| `stream` | **采集器**：自己按 `intervalMs` 循环 | 被**保持运行**，持续产出，直到被要求停止 |

> **调度与监控不属于本协议。** "Host 怎么按时调用 `oneshot`"、"`stream` 进程挂了谁拉起、
> 怎么退避、怎么判断健康"——这些是 Host 的**内部运行机制**，由 Host 的实现文档规定，
> 不写在这里。本协议只规定：**两种模式各自的消息与进程行为是什么**，
> 以保证任何一个 Host 实现都能和任何一个采集器实现正确对接。

### 4.1 `oneshot`：被调用一次

```
Host: spawn → 写请求(stdin) → 关闭 stdin
Collector: 采集 → 输出消息 → 退出
Host: 读完 stdout → 等进程退出 → 按退出码处理
```

* **Host 负责按时调用**：下一轮什么时候开始是 Host 的事，采集器不需要知道，
  也不应该自己 sleep 等待下一轮——**采集器只在被调用时工作一次**。
* 请求里的 `intervalMs` 在 `oneshot` 下 **MAY** 缺省（采集器不需要它）。
* 采集器 **MUST NOT** 假定自己会被复用：**每一次调用都是一个全新的进程**，
  任何"上次运行留下的东西"都不存在（跨调用的记忆只能通过 `state`，见 §5）。

### 4.2 `stream`：被保持运行

```
Host: spawn → 写请求(stdin，含 intervalMs) → 保持 stdin 打开
Collector: 循环 { 采集 → 输出一批消息 → 等待 interval }
Host: 持续读取，边读边落库
结束: Host 关闭 stdin（或发信号）→ Collector 应尽快退出
```

* **采集器负责节奏**：自己按 `intervalMs` 循环产出。`intervalMs` 在 `stream` 下**必填**。
* **Host 负责保持它运行并持续读取**，不主动打断；只在需要结束时关闭 stdin 或发信号。
* 同一个采集器的 `stream` 实例 **MUST** 全局唯一（见 §4.3）。
* 采集器 **MUST NOT** 在被要求停止后继续产出：收到 stdin 关闭或信号后应尽快退出（§4.4）。

**`intervalMs` 的语义**：两轮开始之间的目标间隔（start-to-start），
不是"本轮结束后再等这么久"。采集器 **SHOULD** 从间隔里扣掉本轮耗时，
否则实际周期会漂成 `intervalMs + 采集耗时`。若本轮耗时已超过间隔，
**SHOULD NOT** 试图追补（连发多轮只会雪上加霜），正常进入下一轮即可。

> **该给自己的采集器选哪种模式**，取决于你的数据是"读一次就有意义"还是"必须靠进程内
> 的上一轮测量算出来"，以及来源是拉还是推。这是**采集器设计**的取舍，
> 判据与实测数据见 Host 侧的运行机制文档（本仓库为 `DESIGN.md` §6.2.13）。

**因此：本地系统类采集器 SHOULD 默认使用 `stream` 模式。**
Host **MUST** 同时支持两种模式，且对存储层而言两者没有区别（都是流式消息）。

### 4.3 并发、串行化与重叠

* 同一 Collector 的**多次运行之间 MUST 串行**——Host **MUST NOT** 在上一次尚未结束时启动下一次。
* 不同 Collector 之间 **MAY** 并发（Host 应有全局并发上限）。
* `stream` 模式下不存在"重叠"，但 Host **MUST** 保证同一采集器只有一个常驻实例。

### 4.4 取消与超时

* Host 超时（`limits.timeoutMs`，`stream` 模式下为单次产出间隔的宽限）后 **MUST** 终止 Collector。
* 终止顺序：关闭 stdin → 发送终止信号（POSIX `SIGTERM`）→ 宽限期（如 5s）→ 强制杀死。
* **终止 MUST 作用于整棵进程树**（采集器可能 fork 子进程）。
  Windows 无 `SIGTERM`，Host **MUST** 用 Job Object 或 `taskkill /T` 达到同等效果。
* Collector **SHOULD** 在 stdin 关闭或收到信号后**尽快退出**（清理、落盘、退出），不得阻塞。
  注意**背压会妨碍这一条**：stdout 是管道，Host 若停止读取，Collector 会阻塞在写上，
  此时它读不到"stdin 已关闭"。因此 Host **MUST** 仍然按上面的顺序超时终止，
  不能只靠"关 stdin 等它自己退"。

### 4.5 资源限制

Host **SHOULD** 施加并在请求的 `limits` 中告知：`timeoutMs`、`maxOutputBytes`、`maxLines`。
超限时 Host **MUST** 终止 Collector 并把该次运行标记为失败，错误信息说明超的是哪一项。
Collector **SHOULD** 自行控制产出量（例如按配置只取 top-N 进程）。

## 5. 状态（跨运行记忆）

采集器常常需要"上次采到哪"（增量游标、上次的计数器快照）。本协议提供两种方式：

1. **`state` 消息 + 请求里的 `state`**：轻量、通用。
   ```json
   {"type":"state","state":{"lastId":11}}
   ```
   * Host **MUST** 只在该次运行**成功**（退出码 0）时保存 `state`；
     失败时保留旧值——否则会**静默跳过一段数据**。
   * `state` **MUST** 是可 JSON 序列化的对象，Host **SHOULD** 限制其大小（如 64 KiB）。
2. **采集器自管状态**（写自己的文件）：**MAY**，但要求采集器自己能处理并发与损坏。

> `stream` 模式下的采集器通常**不需要** `state`（进程内保留上一次测量即可）。
> `state` 主要服务 `oneshot` 模式。

---

## 6. 错误模型

### 6.1 错误类别

| `kind` | 含义 | 可重试 | 面向用户的处置 |
| --- | --- | --- | --- |
| `config` | 配置写错 | 否 | 停用并提示修改 |
| `auth` | 凭据无效/过期 | 否 | 提示更新凭据（不要反复打上游） |
| `network` | 网络不可达/超时 | 是 | 退避重试 |
| `upstream` | 上游 5xx / 限流 / 依赖缺失 | 是 | 退避重试 |
| `parse` | 上游格式变了或解析失败 | 否（或低频重试） | 提示用户 |
| `internal` | 采集器自身 bug | 是 | 退避重试并记录 |

**把"可重试"与"要告诉用户"分开**，才能既不浪费重试，也不把需要用户修的问题埋进日志。

### 6.2 部分成功语义

* Collector **MAY** 先输出若干消息、再失败退出。
* Host **MUST**保留**已成功解析并通过校验**的消息（写入是幂等的，见 §7），
  同时把该次运行标记为失败、**不提交 `state`**。
* 因为写入幂等，重试时重复产出同一 `(name, ts)` 是安全的。

---

## 7. 幂等与去重（Host 职责）

* 数值：同一 `(series name, ts)` 只有第一次写入生效（`INSERT OR IGNORE` 语义），
  **已存在时 MUST NOT 覆盖**（同一时刻的重复采集是重放，不是修正）。
* 记录：给出 `key` 时，同一 `(kind, key, ts)` 去重；未给出 `key` 时每条独立。
* 因此 Collector **可以**安全地重发同一批数据（网络抖动、重试、进程重启）。
* 时间戳精度：Host **SHOULD** 明确一个最小时间粒度（建议毫秒），
  更细的精度会被截断，可能导致同毫秒内的多个点相互覆盖。

---

## 8. 元数据与发现

### 8.1 自描述

Collector **MUST** 能通过 **`--describe`** 输出一行 JSON 描述自身：

```json
{
  "protocol": 1,
  "kind": "system",
  "title": "本机系统信息",
  "version": "0.1.0",
  "modes": ["oneshot", "stream"],
  "defaultMode": "stream",
  "defaultIntervalMs": 2000,
  "output": "samples",
  "recordKinds": [
    { "kind": "system.process", "help": "进程榜快照",
      "persist": true, "maxRetentionMs": 3600000 }
  ],
  "configSchema": {
    "type": "object",
    "properties": {
      "intervalMs": { "type": "integer", "title": "采样间隔(ms)", "default": 2000, "minimum": 250 },
      "categories": { "type": "array", "title": "采集类别",
                      "items": { "enum": ["cpu","memory","disk","network","temperature","gpu","processes","system"] } },
      "topProcesses": { "type": "integer", "title": "进程 Top N", "default": 5, "minimum": 0 }
    }
  }
}
```

`record` **没有** `unit` / `help`（见 §2.3），所以字段名要承担这部分信息：

```json
{"type":"record","kind":"system.process","key":"1234",
 "payload":{"pid":1234,"name":"nginx","cpuUsagePercent":37.5,
            "memoryBytes":104857600,"runTimeSeconds":86400,"status":"Sleep"}}
```

* **带量纲的字段名 SHOULD 自带单位**：用 `memoryBytes` 而不是 `memory`，
  用 `runTimeSeconds` 而不是 `runtime`。否则宿主只能猜"这是字节还是 MB"，
  而猜错是**看不见的**错误（数字照样画出来）。
* **同名但口径不同的量 MUST 在 `kind` 的文档里说明**，且**同一个 `record` 内不得混用**。
  上例的 `cpuUsagePercent` 就是这种情况：它是**逐进程口径**
  （分母等价于墙钟时间，多核上可超过 100，实测一个 4 线程进程报 403.7%），
  与 `system/cpu_usage` 的系统口径（0–100）不是同一个量。见 §11.1。

> 这条规则**不要求**字段名长到能自我解释一切（`cpuUsagePercent` 是可接受的），
> 因为 `record` 的字段是采集器自定义的、平台无法预先枚举；
> 它要求的是"**不要把有歧义的口径藏着不写**"。

* `recordKinds`（可选）：该采集器会产出的**记录类型**清单，每项为
  `{kind, help?, persist?, maxRetentionMs?}`——字段与 `series` 的保留声明完全一致（§2.8）。
  它存在的意义是**记录类型此前没有任何声明渠道**：宿主拿到一条 `record` 只能看到 `kind` 字符串，
  既不知道它是干什么的，也不知道该留多久。未列出的类型仍然合法（类型由用户配置决定时无法预先声明），
  宿主按数据模型使用默认策略。
* `configSchema` 为 **JSON Schema**，Host 据此**自动生成配置表单**（因此新增采集器不需要写前端代码）。
* 也 **MAY** 在工作目录放置 `collector.json`（同结构），便于不带命令行参数地登记。
  **两者同时存在时以实现（`--describe`）为准**，避免漂移。

### 8.2 能力声明

`modes` / `output` / `configSchema` 是 Host 决定"能否调度、如何展示、怎么配置"的依据：

* `modes` / `defaultMode`：采集器**支持**哪些运行模式（§4）、缺省用哪个；
* `output`: `samples` | `records` | `both`；
* Host **MUST** 在采集器声明不支持某模式时拒绝以该模式启动，并给出明确错误。

---

## 9. 版本协商与兼容

* `protocol` 为主版本号；不兼容变更 **MUST** 递增。
* Collector **MUST** 在请求的 `protocol` 与自身不兼容时，输出一条 `error`（`kind: config`）并以退出码 `2` 退出，
  **不得"尽力而为"地继续**。
* Host **SHOULD** 支持当前主版本与上一主版本（过渡期）。
* 新增**可选**字段不递增主版本；接收方 **MUST** 忽略未知字段。

---

## 10. 一致性测试（Conformance）

Host **SHOULD** 提供 `collector test <path>` 校验一个采集器是否符合本标准，至少覆盖：

| # | 检查项 |
| --- | --- |
| 1 | `--describe` 输出合法 JSON，且含 `protocol/kind/configSchema` |
| 2 | 收到请求后能产出至少一条消息并正常退出（退出码 0） |
| 3 | stdout 中**每一行**都是合法 JSON 对象且 `type` 已知（**这是最重要的一项**） |
| 4 | `sample.name` 符合 §2.4 命名规则；`value` 为数值 |
| 5 | 声明过的 `series` 语义一致；未声明的 `sample` 给出警告 |
| 5b | `persist` / `maxRetentionMs` 取值合法（`persist:false` 时不得同时给 `maxRetentionMs`；`maxRetentionMs` 为正整数或 `null`）；`record` 的 `kind` 在 `recordKinds` 中声明过（未声明给出**警告**，不是错误） |
| 6 | 非法配置下以退出码 `2` 退出，并给出可读错误（**不得**崩溃栈） |
| 7 | 超时能被终止（含子进程）；关闭 stdin 后及时退出 |
| 8 | 产出量遵守 `limits`（或能被 Host 正常截断） |
| 9 | 消息**及时可见**（验证 flush 行为：产出第一条后 1 秒内 Host 应能读到） |
| 10 | stderr 中的日志不影响 stdout 数据流 |

第 3 与第 9 项对应最容易出问题的两处：**数据流污染**与**缓冲不 flush**。

**现成工具**（本仓库内，只依赖 Python 标准库）：

```console
$ python3 examples/collectors/conformance.py -- ./target/debug/viacli-sys
$ python3 examples/collectors/conformance.py -- python3 examples/collectors/echo_metrics.py
结果：42/42 项通过
✅ 该采集器符合 docs/collector-protocol.md §10 的一致性要求
```

它把上表逐条变成断言，并额外检查"声明了几种模式就都要能跑通"。
仓库里还有一个**故意违规**的采集器作为检查器自身的回归测试：

```console
$ python3 examples/collectors/conformance.py -- python3 examples/collectors/_nonconformant_fixture.py
结果：22/40 项通过      # 期望就是"大面积失败"——检查器必须抓得出来
```

第 9 项的判据值得单独说明：**不能**只看"够不够快"，因为不 flush 的采集器在 `oneshot`
下会因为"反正要退出"而看起来完全正常（缓冲区在退出时被刷出）。所以检查器在 `stream`
模式下把 stdin 开满 1 秒再关，要求消息**在我们要求它停止之前**就出现——
这才是"flush 了"与"没 flush"唯一可靠的分界。

---

## 11. 参考实现：本地系统采集器（`system`）

这是本标准的第一个实现，也是"验证标准是否好用"的手段。

### 11.1 采集内容

| 类别 | 序列（前缀） | 单位 / `kind` | 数据来源（跨平台） |
| --- | --- | --- | --- |
| **CPU** | `system/cpu_usage`、`system/cpu_usage/core/N` | percent / gauge | 两次刷新的差值 |
| | `system/cpu_frequency`、`system/cpu_frequency/core/N` | hertz / gauge | 每核频率（平台不提供时不产出） |
| | `system/cpu_count` | count / gauge | 逻辑 CPU 数 |
| | `system/load_average/1m`、`/5m`、`/15m` | count / gauge | Windows 上恒为 0 → 不产出 |
| **内存** | `system/memory_total_bytes`、`system/memory_used_bytes`、`system/memory_available_bytes` | bytes / gauge | |
| | `system/swap_total_bytes`、`system/swap_used_bytes` | bytes / gauge | 无 swap 时整组不产出 |
| **磁盘** | `system/disk/<mount>/total_bytes`、`/used_bytes`、`/available_bytes` | bytes / gauge | 挂载点容量 |
| | `system/disk/<mount>/usage` | percent / gauge | |
| | `system/disk/<mount>/read_bytes`、`/written_bytes` | bytes / **counter** | 累计 IO（仅 Linux） |
| | `system/disk/<mount>/read_rate`、`/write_rate` | bytes_per_second / gauge | 由上一轮差值求得 |
| **网络** | `system/net/<if>/rx_bytes`、`/tx_bytes` | bytes / **counter** | 累计流量 |
| | `system/net/<if>/rx_packets`、`/tx_packets` | count / **counter** | |
| | `system/net/<if>/rx_errors`、`/tx_errors` | count / **counter** | |
| | `system/net/<if>/rx_drops`、`/tx_drops` | count / **counter** | 仅 Linux |
| | `system/net/<if>/rx_rate`、`/tx_rate` | bytes_per_second / gauge | 由上一轮差值求得 |
| **温度** | `system/temperature/<label>` | celsius / gauge | hwmon / 平台接口 |
| | `system/temperature/<label>/max`、`/critical` | celsius / gauge | 平台提供时才产出 |
| **GPU/显存** | `gpu/nvidia/<idx>/utilization`、`/memory_utilization` | percent / gauge | **NVML（NVIDIA）**；AMD/Intel 见 11.4 |
| | `gpu/nvidia/<idx>/memory_used_bytes`、`/memory_total_bytes` | bytes / gauge | |
| | `gpu/nvidia/<idx>/temperature_celsius`、`/power_watts` | celsius / watts / gauge | |
| **系统** | `system/uptime_seconds` | seconds / gauge | |
| | `system/process_count` | count / gauge | 进程总数 |
| **系统信息** | `record`：`kind=system.info`，`key=system` | `record` | 主机名/系统版本/CPU 型号/物理核数/**开机时刻**等静态属性 |
| **进程** | `record`：`kind=system.process`，`key=<pid>` | `record` | Top-N 进程快照（理由见下）。<br>其中 `cpuUsagePercent` 是**逐进程口径**（可超过 100，多核累加），<br>与 `system/cpu_usage` 的系统口径（0–100）不是同一个量 |

**`<mount>` 位置放的是挂载点，不是设备名**（`/` → `root`，`/var/lib/docker` → `var-lib-docker`）。
理由：**挂载点在系统内唯一，设备名不是**——同一个设备可以被 bind mount 到多处，
用设备名会让两条不同的数据流撞进同一个序列名。设备名与文件系统类型放在
`series.labels` 里（`device` / `mount` / `fileSystem`），展示时不会丢。

**进程为什么是 `record` 而不是 `sample`**：若把 PID 编进序列名
（`system/process/<pid>/cpu_usage`），序列基数就等于"历史上出现过的所有 PID"——
短命进程（编译器、shell、容器 init）每天产生成千上万个序列，维度表无界增长，
而"按 PID 画折线"几乎没有人真的想看。Top-N 本质是**某一时刻的一张进程榜**，
正是 `record` 的语义：`kind` 分组、`key` 是稳定标识（PID）、`payload` 结构化
（含可读的进程名、内存、运行时长、状态）。`system.info` 同理：
主机名与系统版本是实体的属性，不是随时间变化的数值。
这条经验对**所有采集器**都适用——**凡是"随实体数量增长的高基数数据"，先考虑 `record`。**

**另一个坑：线程不是进程。** Linux 上把线程也列进进程表是很常见的默认行为
（`sysinfo` 的 `ProcessRefreshKind::nothing()` 里就是 `tasks: true`），
实测后果是：一个 4 线程的忙进程会**独占整个 Top-N**（主进程 400.5% 加 4 个线程各约 100%），
`process_count` 也从 4 虚高到 10；按行累加还会把它的 CPU 重复计算一遍。
采集器**应当只列线程组组长**。注意"只列组长"不会漏掉 CPU：
`/proc/<pid>/stat` 报的本来就是**整个线程组**的累计时间
（实测 4 个忙线程 → 该进程读数 400.5% ≈ 4 核）。

### 11.2 两种模式

* **`stream`（默认）**：常驻，按 `intervalMs`（缺省 2000，最小 250）循环产出。
  CPU/网络/磁盘速率由**进程内保留的上一轮测量**求出；因此 `*_rate` 系列**只在 `stream` 出现**，
  且每轮的读数语义是**该间隔上的平均值**。
  每轮的 `ts` = 请求里的 `now` + 进程内的单调流逝时间：基准来自宿主（不算"自己造时间"），
  又不会在长跑之后与宿主时钟漂移（代价见 11.4）。
* **`oneshot`**：输出**不依赖时间差**的量（内存、磁盘容量、温度、计数、累计 counter），
  **不产出任何 `*_rate` 系列**（没有上一轮测量）。
  CPU 使用率通过**短间隔双采样**（构造时一次 + 250ms 后一次）求出，
  读到的是那 250ms 窗口上的**瞬时值**——与 `stream` 同名的 `system/cpu_usage` 语义不同，
  这是已知的取舍，见 §11.4。

### 11.2.1 保留声明（实测值）

本采集器按 §2.8 声明每类数据的保留上限——**宿主无法自己推断这件事**：

| 保留上限 | 覆盖的序列 | 理由 |
| --- | --- | --- |
| **1 天** | `system/cpu_usage*`、`cpu_frequency*`、`load_average/*`、所有 `*_rate`、GPU 利用率 | 3 秒粒度的抖动读数；再长的时间跨度该靠降采样，不是堆 raw |
| **7 天** | 内存/swap/显存、温度、`uptime_seconds`、`process_count` | 变化慢、看趋势有意义 |
| **30 天** | 磁盘容量、磁盘 IO 计数器、网络计数器 | **长期趋势的原料**：用户把全局保留调长时，最值得留下的就是这些 |
| **永久** | `record: system.info`（主机身份） | **不可再生**——丢了就再也拿不回来 |
| **1 天** | `record: system.process`（进程榜快照） | 一小时前的进程榜基本没人看 |

### 11.3 降级策略（重要）

* 某个类别**取不到数据**（无权限、虚拟化环境无传感器、无 GPU 驱动）时：
  **不报错、不产出该序列**。整体运行仍算成功。
* 只有在**所有**类别都取不到时，才以 `error(kind: upstream)` + 退出码 `4` 报告。
  判定依据是"本轮是否产出了至少一条数据消息（`sample` 或 `record`）"——
  只数 `sample` 会把"只产出 `record` 的类别"误判成全空。
* 拼出来**名字非法**的序列（例如超长挂载点，或含空格/中文的传感器标签）
  **整条丢弃并只警告一次**，不影响其余数据：让一个畸形挂载点连带废掉整轮采集是不可接受的。
* 理由：虚拟机上没有温度传感器、容器里没有 GPU，是**常态而非故障**；
  让整次采集失败会掩盖其它本来可用的数据。

### 11.4 已知限制（诚实声明）

* **GPU 显存/温度**：首版仅支持 **NVIDIA（NVML）**。AMD（`amdgpu` sysfs / ADL）与
  Intel（i915 sysfs）需要各自的实现，列为后续工作。
* **温度**：Linux 走 `/sys/class/hwmon`；虚拟环境通常为空。macOS/Windows 依赖平台接口，覆盖不全。
* **磁盘累计 IO 与网络丢包仅 Linux 提供**（`/proc/diskstats`、`/proc/net/dev`）。
  这两项 `sysinfo` 0.39 没有暴露，所以其它平台**不产出**这些序列，
  而不是用 0 冒充一个看起来正常的读数（见 11.3）。
* **扇区换算**：`/proc/diskstats` 的扇区计数按固定 512 字节换算，与设备物理扇区大小无关
  （这是内核约定，不是估算）。
* **温度标签去重**：多路 CPU 会出现同名传感器（如两处 `Package id 0`），
  第二个及以后会加 `-2`、`-3` 后缀；`series.labels.label` 里保留原始名字。
* **同一个 `system/cpu_usage` 在两种模式下的语义不同**：`stream` 是整个间隔的平均值，
  `oneshot` 是 250ms 窗口的瞬时值。**这是本参考实现的已知取舍，不是推荐做法**——
  更干净的做法是把 CPU 累计时间以 `counter` 发出、由宿主按 `Δ忙碌/Δ总时间` 求比值，
  那样两种模式的语义就一致了（当前未实现）。
* **产出量对"网卡数量"极其敏感**：每个网卡贡献 8 条序列。实测一台普通开发机上
  **31 个网卡里有 27 个是 Docker 的 `br-*` 网桥**——默认全采时共 335 条序列，
  其中 216 条是容器网桥的噪声，按 3 秒间隔约合 **1.7 GB / 7 天**。
  默认的 `excludeInterfaces` 因此排除回环与容器/虚机/隧道网卡，
  实测把序列数从 **335 降到 102**（约 0.4 GB / 7 天）。
  要监控网桥时用 `includeInterfaces` 显式打开。**这是本项目里最大的一处存储节省，
  远大于保留声明带来的收益**（见下一条）。
* **保留声明能收窄，但收窄不了数量级**：按 §11.5 的 `maxRetentionMs` 声明，
  全局默认 7 天下保留量从 20.6M 点降到 15.6M 点（**-24%**）；全局调到 30 天时是 **-52%**。
  也就是说保留声明做的是"**让每类数据的保留期符合它的性质**"，
  真正决定存储量级的是**序列数量 × 采样频率**——
  要压量级得靠收窄 `categories`/`includeInterfaces`，或靠 rollup（D23 未实现）。
* **`stream` 的 `ts` 不跟随 NTP 校正**（它基于宿主 `now` + 单调时钟）。
  单次运行通常远短于时钟校正的影响；长时间运行后若发现偏移，重启采集器即可重新对齐。
* **磁盘列表会出现 bind mount 噪声**：snap 等机制会把同一个设备挂到
  `/var/snap/<pkg>/...`，它们与根分区共享同一份 IO 计数。
  用 `excludeDisks` 按子串排除即可（`series.labels.device` 会显示真实设备）。
* 容器（Docker/WSL）中宿主硬件信息不可见，属预期行为。

### 11.5 配置项

```json
{
  "intervalMs": 2000,
  "categories": ["cpu", "memory", "disk", "network", "temperature", "gpu", "system"],
  "topProcesses": 5,
  "includeInterfaces": [],
  "excludeInterfaces": ["lo", "docker", "veth", "br-", "virbr", "tun", "tap", "wg", "zt"],
  "includeDisks": [],
  "excludeDisks": []
}
```

---

## 附录 A：完整交互示例（oneshot）

下面是**一次真实运行的原样输出**（不是手写示意），主机为 Ubuntu 24.04 / Ryzen 5 PRO 4650G。
为了让篇幅可控，`config.categories` 只选了 `memory`/`system`/`processes`：

```console
$ echo '{"protocol":1,"runId":"r1","now":1799999999123,"mode":"oneshot",
         "config":{"categories":["memory","system","processes"],"topProcesses":1}}' \
    | viacli-sys
{"collector":"system","type":"meta","version":"0.1.0"}
{"key":"system","kind":"system.info","payload":{"bootTimeSeconds":1789528462,"cpuArch":"x86_64","cpuBrand":"AMD Ryzen 5 PRO 4650G with Radeon Graphics","cpuVendor":"AuthenticAMD","distributionId":"ubuntu","hostName":"ubuntu-A520I-AC","kernelVersion":"6.14.0-29-generic","logicalCpuCount":12,"osName":"Ubuntu","osVersion":"Linux (Ubuntu 24.04)","physicalCoreCount":6},"ts":1799999999123,"type":"record"}
{"help":"物理内存总量","kind":"gauge","name":"system/memory_total_bytes","type":"series","unit":"bytes"}
{"name":"system/memory_total_bytes","ts":1799999999123,"type":"sample","value":33515589632}
{"help":"已用内存","kind":"gauge","name":"system/memory_used_bytes","type":"series","unit":"bytes"}
{"name":"system/memory_used_bytes","ts":1799999999123,"type":"sample","value":12300242944}
{"help":"可用内存（含可回收缓存）","kind":"gauge","name":"system/memory_available_bytes","type":"series","unit":"bytes"}
{"name":"system/memory_available_bytes","ts":1799999999123,"type":"sample","value":21215346688}
… （swap / 进程总数等略）…
{"help":"系统运行时长","kind":"gauge","maxRetentionMs":604800000,"name":"system/uptime_seconds","type":"series","unit":"seconds"}
{"name":"system/uptime_seconds","ts":1799999999123,"type":"sample","value":301430}
{"key":"4","kind":"system.process","payload":{"cpuUsagePercent":0.0,"memoryBytes":3719168,"name":"viacli-sys","parentPid":2,"pid":4,"runTimeSeconds":0,"status":"Run","virtualMemoryBytes":4993024},"ts":1799999999123,"type":"record"}
{"level":"info","message":"oneshot 采集完成（run=r1），产出 10 条数据（sample + record）","type":"log"}
```

可以观察到几件事：

* **JSON 对象的键是排好序的**（`serde_json` 的 `Map` 默认按字典序）——
  协议不要求键的顺序，宿主不应依赖任何顺序。
* 每条 `sample` 都带上了 `ts`（= 请求里的 `now`）。参考实现选择**显式携带**而不是省略，
  这样把一批消息直接存进文件也仍然是自解释的。
* `record` 只出现了一条 `system.process`（`topProcesses: 1`），
  它的 `key` 是 PID 的字符串形式；**PID 没有出现在任何序列名里**（理由见 11.1）。
* 最后一条是 `log`，走的是 stdout 的协议消息（不是普通的打印语句）。
  人类可读的日志同时写到了 stderr，两者互不干扰。

## 附录 B：字段速查

| 消息 | 必需字段 | 可选字段 |
| --- | --- | --- |
| 请求 | `protocol`,`runId`,`now`,`mode`,`config` | `intervalMs`,`state`,`limits`,`collector`,`datasource`,`cwd` |
| `meta` | `collector` | `version` |
| `series` | `name`,`kind` | `unit`,`help`,`labels`,`persist`,`maxRetentionMs` |
| `sample` | `name`,`value` | `ts` |
| `record` | `kind`,`payload` | `key`,`ts` |
| `state` | `state` | — |
| `log` | `message` | `level` |
| `error` | `kind`,`message` | `retryable` |
