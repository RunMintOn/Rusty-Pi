# Write file tool — `write`

**What to build:** `write` 工具，写入内容到文件。对齐原版 `@earendil-works/pi-coding-agent/src/core/tools/write.ts` 全部行为。

- 自动创建父目录（`mkdir -p` 语义）
- 文件不存在创建，存在覆盖
- 返回写入确认：`Successfully wrote N bytes to path`
- 文件变动队列（`withFileMutationQueue`）——对同一真实路径串行化写入，避免并发竞态
- abort signal 支持（通过 signal 检查点而非事件监听器，以保持 mutation queue 锁定至操作完成）
- `WriteOperations` 接口可插拔（默认实现为本地文件系统，将来可替换为 SSH 等远程写入）

**Blocked by:** None — can start immediately.

- [ ] `WriteParams` 类型定义（path: String, content: String）
- [ ] `file-mutation-queue` 共享模块实现
- [ ] `WriteTool` 实现 `Tool` + `AgentTool` trait
- [ ] 写入逻辑：创建父目录 → 写入文件 → 返回确认
- [ ] abort signal 支持（非事件监听器模式，而是 await 后检查）
- [ ] 注册到 `main.rs`
- [ ] 测试：写入新文件、覆盖已有文件、自动创建目录、父目录不存在时创建、写入权限错误、abort
