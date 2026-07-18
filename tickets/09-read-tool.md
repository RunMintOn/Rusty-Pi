# Read file tool — `read`

**What to build:** `read` 工具，读取文件内容并返回给 LLM。对齐原版 `@earendil-works/pi-coding-agent/src/core/tools/read.ts` 全部行为。

支持文本文件和图片文件：

- **文本文件**：使用 `truncateHead` 策略截断（2000 行 / 50KB 限制），支持 `offset`（1-indexed）和 `limit` 参数做部分读取
- **图片文件**（jpg/png/gif/webp/bmp）：检测 MIME type，自动缩放（2000x2000 max），以 image content block 返回
- 截断时给出清晰的可操作提示：`[Showing lines N-M of total. Use offset=P to continue.]`
- 路径解析：`~` 展开、`@` 前缀、macOS NFD/AM-PM curly-quote 容错
- abort signal 支持
- `ReadOperations` 接口可插拔（默认实现为本地文件系统，将来可替换为 SSH 等远程读取）

需将截断逻辑提取到共享模块（当前 `truncate_tail` 在 `coding_agent/tools/bash.rs`，需新增 `truncate_head`）。

**Blocked by:** None — can start immediately.

- [ ] `ReadParams` 类型定义（path: String, offset: Option<usize>, limit: Option<usize>）
- [ ] 共享 `truncate` 模块：提取 `truncate_tail`、新增 `truncate_head`（2000 行 / 50KB，保持完整行）
- [ ] `ReadTool` 实现 `Tool` + `AgentTool` trait
- [ ] 文本文件读取：完整读取 → 按 offset/limit 切片 → truncate_head → 拼接 continuation 提示
- [ ] 图片文件读取：MIME 检测 → 读取 → image content block 返回
- [ ] macOS 路径容错（NFD、AM/PM、curly-quote）
- [ ] 注册到 `main.rs`
- [ ] 测试：文本读取、offset/limit、截断、不存在的文件、图片检测、abort
