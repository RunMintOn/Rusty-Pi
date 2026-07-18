# Rusty-pi: 原版功能摸底 & 低成本对齐分析

按原版 5 个 package 逐模块梳理。每个功能标注：
- **行数**：原版 TS 源文件行数（含测试会特别注明）
- **状态**：✅ 已实现 / ⚠️ 部分实现 / ❌ 未实现
- **可测性**：按我们 100% 纯断言原则评估

---

## 1. AI Provider 系统（packages/ai）

> 用户已排除：暂不移植更多 provider。此表仅用于完整性。

| 模块 | 行数 | 状态 | 可测性 | 备注 |
|---|---|---|---|---|
| Provider 架构（trait + types + streaming） | ~1,200 | ✅ | 100% | 已实现 |
| DeepSeek provider | ~15 | ✅ | 100% (mock) | 已实现 |
| OpenAI Codex provider | ~1,600 | ✅ | 100% (mock) | 已实现 |
| Anthropic provider | ~20 | ❌ | 100% (mock) | 排除 |
| Google provider | ~15 | ❌ | 100% (mock) | 排除 |
| Mistral provider | ~15 | ❌ | 100% (mock) | 排除 |
| 其他 30+ provider | ~15-100 每个 | ❌ | 100% (mock) | 排除 |
| OAuth 认证流程 | ~1,500 | ⚠️ 仅 Codex | 100% (mock) | 排除 |
| 模型列表 / 模型注册 | ~800 | ⚠️ 部分 | 100% | 小量工作 |
| thinking 层级支持 | ~200 | ❌ | 100% | 小量工作 |

---

## 2. Agent 核心（packages/agent）

| 模块 | 行数 | 状态 | 可测性 | 备注 |
|---|---|---|---|---|
| Agent loop | ~700 | ✅ | 100% | 已实现 |
| Session 树（id/parentId/branching） | ~340 | ✅ | 100% | 已实现 |
| Session 持久化（JSONL） | ~180 | ✅ | 100% | 已实现 |
| Compaction | ~750 | ✅ | 100% | 已实现 |
| Skills | ~375 | ✅ | 100% | 已实现 |
| System prompt | ~35 | ✅ | 100% | 已实现 |
| Prompt templates | ~270 | ✅ | 100% | 已实现 |
| 消息类型 | ~800 | ✅ | 100% | 已实现 |
| 事件系统（EventBus） | ~35 | ❌ | 100% | 需要评估 |
| Retry 逻辑 | ~150 | ❌ | 100% | 需要评估 |

---

## 3. Coding Agent（packages/coding-agent）— 重点

### 3.1 CLI 参数

| 参数 | 原版 | 已实现 | 可测性 | 工作量 |
|---|---|---|---|---|
| `--provider` | ✅ | ✅ | 100% | ✅ |
| `--model` | ✅ | ✅ | 100% | ✅ |
| `--resume / -r` | ✅ | ❌ | 100% | 小（ticket 14） |
| `--session / --session-id` | ✅ | ❌ | 100% | 小 |
| `--continue / -c` | ✅ | ❌ | 100% | 小 |
| `--fork` | ✅ | ❌ | 100% | 中 |
| `--print / -p`（单次模式） | ✅ | ⚠️ 部分 | 100% | 小 |
| `--mode text / json / rpc` | ✅ | ❌ | 100% | 小 |
| `--name / -n` | ✅ | ❌ | 100% | 极小 |
| `--no-session` | ✅ | ❌ | 100% | 极小 |
| `--list-models` | ✅ | ❌ | 100% | 小 |
| `--thinking` | ✅ | ❌ | 100% | 小 |
| `--tools / -t` (allowlist) | ✅ | ❌ | 100% | 小 |
| `--exclude-tools / -xt` | ✅ | ❌ | 100% | 小 |
| `--no-tools / --no-builtin-tools` | ✅ | ❌ | 100% | 小 |
| `--verbose` | ✅ | ❌ | 100% | 极小 |
| `--offline` | ✅ | ❌ | 100% | 极小 |
| `--api-key` | ✅ | ❌ | 100% | 极小 |
| `--system-prompt` | ✅ | ❌ | 100% | 极小 |
| `--append-system-prompt` | ✅ | ❌ | 100% | 小 |
| `@file` 文件参数 | ✅ | ❌ | 100% | 小 |
| `--export <file>` (HTML) | ✅ | ❌ | 100% | 中 |
| extensions 相关参数 | ✅ | ❌ | 排除 | 排除 |

### 3.2 工具（Tools）

| 工具 | 行数 | 状态 | 可测性 | 工作量 |
|---|---|---|---|---|
| bash | ~470 | ✅ | 100% | ✅ |
| read | ~350 | ✅ | 100% | ✅ |
| write | ~270 | ✅ | 100% | ✅ |
| edit | ~440 | ✅ | 100% | ✅ |
| truncate（工具） | ~280 | ✅ | 100% | ✅ |
| **find** | ~375 | ❌ | 100% | 中 |
| **grep** | ~385 | ❌ | 100% | 中 |
| **ls** | ~225 | ❌ | 100% | 中 |
| **edit-diff** | ~560 | ❌ | 100% | 中 |
| file-mutation-queue | ~60 | ❌ | 100% | 小 |
| output-accumulator | ~220 | ❌ | 100% | 小 |
| path-utils | ~120 | ❌ | 100% | 小 |

### 3.3 核心基础设施

| 模块 | 行数 | 状态 | 可测性 | 工作量 |
|---|---|---|---|---|
| Session Manager | ~1,623 | ❌ | 100% | 大 |
| Settings Manager | ~1,234 | ❌ | 100% | 大 |
| Model Resolver | ~705 | ❌ | 100% | 中 |
| Model Registry | ~287 | ❌ | 100% | 中 |
| Model Runtime | ~587 | ❌ | 100% | 中 |
| Resource Loader | ~1,040 | ⚠️ 部分 | 100% | 大 |
| Config 系统（config.ts） | ~566 | ❌ | 100% | 中 |
| Provider Composer | ~548 | ❌ | 100% | 中 |
| Trust Manager | ~244 | ❌ | 100% | 小 |
| Output Guard | ~108 | ❌ | 100% | 小 |
| Cache Stats | ~156 | ❌ | 100% | 小 |
| Provider Attribution | ~97 | ❌ | 100% | 小 |
| Runtime Credentials | ~48 | ❌ | 100% | 小 |

### 3.4 排除项（不可 100% 测试 / 明确排除）

| 模块 | 行数 | 原因 |
|---|---|---|
| 扩展系统（loader/runner/types/wrapper） | ~3,500 | 涉及 TS 运行时，不可 100% 测 |
| 交互式模式（interactive-mode.ts） | ~6,008 | TUI，我们不走这条路线 |
| 35 个 TUI 组件 | ~6,000 | TUI，同上 |
| Theme 系统 | ~1,300 | TUI 附属 |
| 包管理器（package-manager.ts） | ~2,650 | 扩展安装，不可 100% 测 |
| Export HTML | ~600 | 可测但非核心，延后 |
| Keybindings | ~370 | TUI 附属 |

---

## 4. TUI 框架（packages/tui）— 排除

全部 ~12,000 行，走裸终端路线，不移植。

---

## 5. Orchestrator（packages/orchestrator）— 延后

| 模块 | 行数 | 状态 | 可测性 | 备注 |
|---|---|---|---|---|
| RPC 协议 | ~142 | ❌ | 100% | 延后 |
| IPC Server/Client | ~270 | ❌ | 100% | 延后 |
| Supervisor | ~354 | ❌ | 100% | 延后 |
| RPC mode | ~795 | ❌ | 100% | 延后 |

---

## 低成本对齐清单（按优先级排列）

> 满足：需要 + 100% 可测 + 工作量合理

### Tier 1 — 缺失工具（高频调用）

| # | 工具 | 工作量 | 原版参考 |
|---|---|---|---|
| 1 | **ls** | ~200 行 | tools/ls.ts (225 行) |
| 2 | **find** | ~350 行 | tools/find.ts (374 行) |
| 3 | **grep** | ~350 行 | tools/grep.ts (385 行) |
| 4 | **edit-diff** | ~500 行 | tools/edit-diff.ts (560 行) |

### Tier 2 — CLI 参数补齐（小改大收益）

| # | 参数 | 工作量 |
|---|---|---|
| 5 | `--print / -p` 单次模式 | ~50 行 |
| 6 | `--list-models` | ~80 行 |
| 7 | `--name / -n` | ~10 行 |
| 8 | `--no-session` | ~10 行 |
| 9 | `--session / --session-id` | ~50 行 |
| 10 | `--continue / -c` | ~30 行 |
| 11 | `--system-prompt / --append-system-prompt` | ~50 行 |
| 12 | `--thinking` | ~80 行 |
| 13 | `@file` 参数 | ~80 行 |

### Tier 3 — 基础设施（可逐步推进）

| # | 模块 | 工作量 | 依赖 |
|---|---|---|---|
| 14 | Model Registry + `--list-models` | ~300 行 | 无 |
| 15 | Output Guard（原始 stdout 保护） | ~100 行 | 无 |
| 16 | Trust Manager（项目信任） | ~200 行 | 无 |
| 17 | Retry 逻辑 | ~150 行 | 无 |
| 18 | thinkng 层级支持 | ~200 行 | Model Registry |

### Tier 4 — 复杂基础设施（需要评估是否值得）

| # | 模块 | 工作量 | 说明 |
|---|---|---|---|
| 19 | Model Resolver | ~700 行 | 模型选择解析链 |
| 20 | Provider Composer | ~550 行 | 多 provider 组合 |
| 21 | Session Manager 完整版 | ~1,600 行 | 完整 session 生命周期 |
| 22 | Resource Loader | ~1,000 行 | 资源发现+加载 |
| 23 | Config 系统 | ~500 行 | 完整配置层级 |

---

## 总结

**当前已对齐：~25%**（agent 核心 + session + skills + 4 工具 + 2 provider）

**低成本可追加（Tier 1-3）：~15%**（ls/find/grep/edit-diff + CLI 参数补齐 + 轻量基础设施）

**复杂但可做（Tier 4）：~15%**（model resolver、session manager 等）

**排除/延后：~45%**（TUI ~18K + 扩展 ~3.5K + 包管理 ~2.6K + 其他 provider ~15K）
