# CLAUDE.md — openbmclapi-rust 代码规范

本文件是本仓库的硬性约定，不是建议。改动代码前先通读；与本文件冲突的写法一律视为缺陷，
必须在本轮内改正或说明理由。

## 0. 门禁与验证纪律

**每次新开会话、动手之前先 `git pull --ff-only`。** release 工作流会把 tag 的版本号
写回 `Cargo.toml`（含 `Cargo.lock`）并推到 main，本地不拉就会在提交或推 tag 时撞车。
拉不动（无凭据等）时**明确说出来**，不要假装拉过就继续。

### 0.1 范围化，不跑全量

全量跑既没有信息量又吃满 CPU，默认禁止。只跑与改动相关的部分，按代价从低到高：

| 目的 | 命令 |
| --- | --- |
| 编译通过 | `cargo check --offline --lib` |
| 只看自己模块的 lint | `cargo clippy --offline --lib`，再按模块路径过滤输出 |
| 只跑自己模块的测试 | `cargo test --offline --lib <module>::` |
| 集成测试（仅当改了对外行为） | `cargo test --offline --test <name>` |
| 收口验收（主 agent） | `cargo fmt --all --check` + `cargo clippy --offline --lib` + 相关测试 |

- 禁止 `--all-targets`：它额外编译测试与示例目标，代价翻倍，且与本次改动无关。
- 禁止「顺手跑一下全量」。
- 只有改动横跨全仓时才跑全量，且由主 agent 在收口时跑一次。
- CI 是自动化的收口点，那里跑全量（含 `--all-targets`）是正确的，不算违规；
  本地的「范围化」纪律针对的是交互式开发，不是 CI。

### 0.2 串行，不并发

同一 `target` 目录同时只允许一个 cargo 进程。出现 `Blocking waiting for file lock`
说明已有人在跑：等它结束，或把验证交给主 agent，**不要**再起第二个。
并发跑门禁会把机器拖卡，且不会更快。

### 0.3 子 agent 的验证纪律

- 子 agent 只验证自己负责的文件/模块，改动完成后**只跑一次**。
- 两个子 agent 的改动范围若有重叠（同一文件、同一 `mod.rs`、同一 `Cargo.toml`），
  **都不许自己跑**，由主 agent 在它们全部结束后统一跑。
- 子 agent 不得跑整仓 `cargo fmt`（会重排别人正在编辑的文件），
  用 `rustfmt --edition 2021 <文件...>` 限定范围。
- 子 agent 不得新增依赖、不得改 `Cargo.toml`。

### 0.4 工具链版本

`rust-toolchain.toml` 把工具链钉死在 `1.94.1`，CI 与 release 都按它自行安装，
不依赖 runner 上碰巧存在的 rustc。本机 `stable` 当前就是 1.94.1，但名字不叫
`1.94.1`，rustup 会因此去下载一个独立工具链；离线或受限环境下用环境变量覆盖：

```powershell
$env:RUSTUP_TOOLCHAIN = "stable"
```

**不要**依赖 `stable` 这个名字做门禁：本机 stable 停在 1.94.1，GitHub runner 的
stable 已经到 1.99.0，clippy 会多出新 lint，本地全绿而 CI 红。升级本机 stable 后
要同步改 `rust-toolchain.toml`。

### 0.5 判定标准

`cargo fmt --check`、`cargo clippy`、`cargo test` 三项必须全绿。本项目要求
**clippy 零警告**；某条 lint 确实不适用时就地 `#[allow(...)]` 并写明理由，不允许留着警告交付。

## 1. 模块化：单文件行数上限

| 行数 | 要求 |
| --- | --- |
| ≤ 300 | 允许 |
| 301 – 350 | 警戒线：必须能说清「为什么不拆」，说不清就拆 |
| > 350 | **必须拆**，不接受例外 |

统计口径：文件全部行数（含注释、`#[cfg(test)]` 声明与空行）。

拆分优先级，从上到下：

1. **按职责**：类型定义 / 纯算法 / 协议编解码 / IO 与网络 / 对外接口实现，各自独立文件。
2. **按数据流**：解析 → 变换 → 输出。
3. 禁止按「前半段 / 后半段」这种无意义方式截断。

拆分后的形态：

```
src/<domain>/
  mod.rs          仅入口（见 §2）
  <part>.rs       一个职责一个文件
  <part>_test.rs  该文件的单元测试（见 §4）
```

`<domain>.rs` 升级为 `<domain>/` 目录后，外部引用路径不变（`crate::<domain>::X`），
由 `mod.rs` 的 `pub use` 保持。

## 2. lib.rs / mod.rs / main.rs 只能是入口

这些文件**只允许**出现：

- 模块文档注释 `//!`
- `mod` / `pub mod` 声明
- `pub use` / `pub(crate) use` 再导出
- 修饰上述声明的 `#[cfg(...)]`

**禁止**在入口文件里定义函数、结构体、枚举、trait、常量、`impl` 块或宏。
出现即结构错误，必须移到 `<domain>/<part>.rs`。

## 3. 复用优先

- 动手前先搜：本仓库是否已有同职责实现（`grep`）；现有依赖是否已提供。
- 同一段逻辑出现第二次必须提取为函数；出现第三次必须提取为模块。
- 跨模块共用的类型放 `types.rs` 或所属 domain 的公共文件，禁止各自复制一份。
- 新增第三方依赖前先问：现有依赖能否覆盖？能就不加。
- 禁止复制粘贴式实现变体，用泛型、trait 或参数化收敛差异。

## 4. 测试布局

| 类型 | 位置 | 引入方式 |
| --- | --- | --- |
| 单元测试（测私有函数、内部状态） | `src/<module>_test.rs` | 在 `<module>.rs` 末尾写 `#[cfg(test)] #[path = "<module>_test.rs"] mod tests;` |
| 集成测试（起真实服务、跨模块） | `tests/<name>.rs` | Cargo 自动发现 |

规则：

- **禁止**在实现文件里内联写测试函数体；测试代码必须落在 `_test.rs`。
- 命名与被测文件同名加 `_test`：`util.rs` → `util_test.rs`；目录模块内就地放
  `storage/s3/sigv4.rs` → `storage/s3/sigv4_test.rs`。
- 一个测试只验证一个行为；测试名用陈述句描述行为，不用 `test_` 前缀。
- 单元测试不得联网、不得依赖外部服务；需要网络或真实文件的放 `tests/`。
- 临时产物见 §4.1。

### 4.1 临时产物

单元测试一律经 `crate::testutil::{TempDir, TempFile}` 创建，不要自己拼路径：

- 根目录是 `std::env::temp_dir()` 下的 `openbmclapi-tests/`，条目名带进程号 +
  递增序号 + 标签，**每个测试唯一**。写死名字（例如 `openbmclapi-config-absent.yaml`）
  会让并行跑的两个测试互相删目录。
- 守卫在 `Drop` 里清理，断言失败也会清干净；最后一个守卫退出时顺手收掉空的根目录。
- **禁止删除进程的当前目录**：Windows 会直接拒绝，`remove_dir_all` 返回错误，
  被 `let _ =` 吞掉就是静默泄漏（曾这样积累近百个残留目录）。需要换工作目录的
  测试，先切回原目录再删，见 `tests/http_surface.rs`。
- `tests/` 下的集成测试无法复用 `cfg(test)` 里的守卫，按同样规则自己处理：
  目录名带进程号，恢复工作目录后再清理。

## 5. 注释

- **言简意赅**。解释「为什么」与约束，不复述代码做了什么。
- `pub` 的类型、函数、trait、字段必须有 `///` 文档注释，一到两行说清用途与关键约束。
- 行内 `//` 只用于非显然的取舍、协议怪癖、外部系统限制。
- **禁止**逐行注释、禁止 `// 定义变量 x` 这类同义复述、禁止注释掉的死代码。
- 模块说明用 `//!` 放在文件顶部，说清这个模块负责什么、边界在哪。
- `TODO` / `FIXME` 必须写成 `TODO(条件): 内容`；无条件说明的一律删掉。
- 注释语言为英文，与标识符风格一致。

## 6. 代码风格

- 命名：类型 `UpperCamelCase`，函数/变量/模块 `snake_case`，常量 `SCREAMING_SNAKE_CASE`。
- 错误：库内用 `thiserror` 定义具体错误类型；应用层用 `anyhow`。可恢复错误禁止
  `unwrap()` / `expect()`（测试或真正的不变量除外，且要写明理由）。
- 异步：只用 `tokio`；禁止在异步上下文做阻塞 IO（用 `tokio::fs`、`spawn_blocking`）。
- 共享状态：优先 `Arc<T>` + 消息传递；必须共享时用 `tokio::sync`，不跨 `.await` 持
  `std::sync` 锁。
- 可见性：默认私有，按需放宽到 `pub(crate)` / `pub(super)`；`pub` 只给真正的对外 API。
- 跨模块的 `impl` 块：结构体字段用 `pub(super)`，`impl` 可分散在同级模块里。
- 禁止 `unsafe`，除非确有必要并在注释里给出安全性论证。

## 7. 工作流与外部版本

- 写或改 `.github/workflows/` 之前，**逐个核实 action 的当前最新 major 版本**再落笔，
  禁止凭记忆写。事故记录：长期写 `actions/upload-artifact@v4` 与
  `actions/download-artifact@v4`，触发 "Node.js 20 is deprecated"，被强制在 Node 24 上运行。
- 2026-10 已核实：`actions/checkout@v7`、`actions/upload-artifact@v7`、
  `actions/download-artifact@v8`、`Swatinem/rust-cache@v2`。此表会过期，用前重新核实。
  现查：`gh api repos/<owner>/<repo>/releases/latest --jq .tag_name`。
- **runner 标签也是版本。** `ubuntu-latest` 会静默升级（2026-10-19 起迁到 Ubuntu 26）。
  本仓所有 ubuntu 任务一律钉 `ubuntu-22.04`：对产物依赖 runner glibc 的那条
  （`*-unknown-linux-gnu`）是硬要求，升级即静默抬高门槛、老发行版直接跑不起来；
  对其余任务是为了可复现，顺带让这条信息性注解彻底消失。想拿新版 runner 当探针时，
  单独留一个任务用 `latest` 并写清理由。
- 用 PowerShell 改文本文件时不要用 `Set-Content -Encoding utf8`：PS 5.1 会写入 BOM，
  而 BOM 会让 `Cargo.toml` 变成非法 TOML（`Swatinem/rust-cache` 直接报
  "Invalid TOML document"）。用 `[System.IO.File]::WriteAllText` 配
  `UTF8Encoding($false)`，或直接用编辑工具。
- 写完回读一遍所有 `uses:` 行，确认没有残留旧 major。
- 同理适用于 `Cargo.toml` 里任何带版本号的依赖：现查，不抄记忆。

## 8. Review 清单

提交前逐条自问：

- [ ] 有没有文件超过 350 行？300 以上的都给出不拆的理由了吗？
- [ ] `lib.rs` / `mod.rs` / `main.rs` 里有没有逻辑代码？
- [ ] 测试是否都在 `_test.rs` 或 `tests/` 里？
- [ ] 有没有第二处相同逻辑没被提取？
- [ ] 公共项是否都有文档注释？注释是否在复述代码？
- [ ] `cargo fmt` / `cargo clippy` / `cargo test` 是否全绿？
