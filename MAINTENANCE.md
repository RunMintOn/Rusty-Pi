# Maintenance

## Provider 配置

优先级：环境变量 → 存储凭据（`~/.config/pi-codex-credentials.json`）→ OAuth 交互登录

- 默认 provider：OpenAI Codex
- 切换 DeepSeek：`--provider deepseek` + `DEEPSEEK_API_KEY`
- 跳过 OAuth 直接使用 token：`OPENAI_CODEX_TOKEN`

## Build & Test

```bash
cd rusty-pi && cargo test && cargo clippy
```

200 tests，全部本地 mock，不碰任何在线 API。

特定模块测试：

```bash
cargo test openai_codex
cargo test deepseek
cargo test bash
```

- bash timeout 测试会打印 kill 日志，无害

## Available Tools

当前注册的 4 个工具实现在 `src/coding_agent/tools/`，每个实现 `Tool` + `AgentTool` trait 后在 `main.rs` 注册。详细行为见各模块源码。

| Tool | 文件 |
|---|---|
| `bash` | `tools/bash.rs` |
| `read` | `tools/read.rs` |
| `write` | `tools/write.rs` |
| `edit` | `tools/edit.rs` |

### 添加新工具

每个工具只需：

1. 在 `tools/` 下新建 `.rs` 文件
2. 实现 `Tool` trait（`name`/`description`/`parameters`）
3. 实现 `AgentTool` trait（`label`/`execute`，可选 `prepare_arguments`/`execution_mode`）
4. 在 `tools/mod.rs` 声明 `pub mod your_tool;`
5. 在 `main.rs` 注册：`let tool = YourTool::new(cwd);` → 加入 `tools: vec![...]`

## Prompt Templates & Skills 系统

Prompt templates 和 skills 是两种用户可配置的资源，通过 `--prompt-path` / `--skill-path` 加载，
或从 `~/.pi/agent/prompts/` / `~/.pi/agent/skills/` 自动发现（`RUSTY_PI_AGENT_DIR` 可覆盖家目录位置）。

### Prompt Templates

`src/coding_agent/prompt_templates.rs` — 从 Markdown 文件加载 `/templateName args` 模板。
支持 bash 风格参数替换（`$1`, `$@`, `${N:-default}`, `${@:N:L}`）。

模板文件放在 `prompts/` 目录下，文件名（不含 `.md`）即模板名。
Frontmatter 支持 `description` 和 `argument-hint`。

### Skills

`src/coding_agent/skills.rs` — 遵循 [Agent Skills 规范](https://agentskills.io) 发现和格式化技能。

发现规则：
1. SKILL.md 作为 skill 根节点，不递归
2. 根目录下的 `.md` 文件
3. 递归子目录找 SKILL.md

Skill frontmatter：
- `name`（必填，或父目录名兜底）：小写字母、数字、连字符，最长 64 字符
- `description`（必填）：最长 1024 字符
- `disable-model-invocation`（可选）：为 `true` 时不注入 system prompt

### System Prompt 构建

`src/coding_agent/system_prompt.rs` — 构建完整 system prompt。
拼接 tools 列表、guidelines、pi 文档引用、skills XML、project context 文件。
`custom_prompt` 可跳过默认模板直接使用自定义文本。

### PromptSession

`src/coding_agent/prompt_session.rs` — 薄 session 层，封装 agent + 展开逻辑。

## Session 模块

会话存储位于 `src/agent/session/`。三层架构：`SessionStorage` trait（抽象后端）← `InMemorySessionStorage` / `JsonlSessionStorage`（具体实现）← `Session`（业务逻辑 API）。详见 `docs/adr/003-session-storage-architecture.md`。

**JSONL 文件格式（v3）**：第一行为 JSON session header（`type: "session"`, `version: 3`, `id`, `timestamp`, `cwd`），后续每行一个 JSON 编码的 `SessionTreeEntry`。文件末尾最近的 entry 决定 `leaf_id`。

### Session 入口 JSON 序列化陷阱

`SessionTreeEntry` 枚举使用 `#[serde(tag = "type")]`。所有内层 struct 的 `entry_type` 字段必须加 `#[serde(skip)]` 且 `EntryTypeTag` 必须实现 `Default`，否则反序列化时报 `duplicate field \`type\``。见 `types.rs` 中的实际用法。

## 参考代码位置

原版 TypeScript 参考代码仅存在于**基础仓库** `pi-rust/reference/earendil-works-pi/`，不在 worktree 中。通过 Git worktree 切换到 ticket 分支后，`reference/` 目录不存在，访问需用基础仓库的绝对路径。

## 已知陷阱

| 陷阱 | 现象 | 原因 |
|---|---|---|
| `reference/` 在 worktree 中不存在 | `ls reference/` 报错 | 参考代码只在基础仓库 |
| test filter 返回 0 测试 | 完整路径过滤不匹配 | 用模块名短名 `cargo test openai_codex` |
| bash timeout 测试日志中有 kill 输出 | 测试打印 `kill: no such process` | 无害，测试预期行为 |
| `Content` 没有 `Display` | 不能 `result.content[0].to_string()` | 必须模式匹配 |

### `Content` 没有 `Display`

`ai::types::Content` 枚举没有实现 `Display`/`ToString`。测试中不能直接 `assert!(result.content[0].to_string()...)`，必须用模式匹配：

```rust
let text = match &result.content[0] {
    Content::Text { text } => text.as_str(),
    _ => panic!("Expected text content"),
};
```

## 更新 Reference

```bash
cd reference/earendil-works-pi
git fetch --depth 1 && git reset --hard origin/main && rm -rf .git
```

## Git 陷阱

### filter-branch 物理删除文件

`git filter-branch --index-filter` 结束后会重置工作树。被 `--index-filter` 从所有 commit 中移除的文件会从磁盘消失。安全做法：

- 备份后再操作，或
- 使用 `git filter-repo`（无此副作用），或
- 不重写历史，直接 `git rm --cached` + 新 commit。
