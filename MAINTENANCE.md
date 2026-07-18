# Maintenance: rusty-pi

构建、测试、诊断指南。所有命令在 `rusty-pi/` 下执行（即 `cd rusty-pi && <command>`）。

## 测试

```bash
# 全量
cargo test

# 按模块名过滤（不要用完整路径语法，嵌套模块不匹配）
cargo test openai_codex
cargo test deepseek
cargo test bash
```

- 所有测试 100% 本地运行，MockProvider 替代真实 LLM
- bash timeout 测试会打印 kill 日志，无害

## Provider 配置

优先级：环境变量 → 存储凭据（`~/.config/pi-codex-credentials.json`）→ OAuth 交互登录

- 默认 provider：OpenAI Codex
- 切换 DeepSeek：`--provider deepseek` + `DEEPSEEK_API_KEY`
- 跳过 OAuth 直接使用 token：`OPENAI_CODEX_TOKEN`

## 参考代码位置

原版 TypeScript 参考代码仅存在于**基础仓库** `pi-rust/reference/earendil-works-pi/`，不在 worktree 中。通过 Git worktree 切换到 ticket 分支后，`reference/` 目录不存在，访问需用基础仓库的绝对路径。

## 已知陷阱

| 陷阱 | 现象 | 原因 |
|---|---|---|
| `reference/` 在 worktree 中不存在 | `ls reference/` 报错 | 参考代码只在基础仓库 |
| test filter 返回 0 测试 | 完整路径过滤不匹配 | 用模块名短名 `cargo test openai_codex` |
| bash timeout 测试日志中有 kill 输出 | 测试打印 `kill: no such process` | 无害，测试预期行为 |
