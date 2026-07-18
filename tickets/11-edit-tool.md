# Edit file tool — `edit`

**What to build:** `edit` 工具，对文件做精确文本替换。对齐原版 `@earendil-works/pi-coding-agent/src/core/tools/edit.ts` 全部行为，包括配套的 `edit-diff.ts` 和路径相关工具。

**核心行为：**
- `edits: [{ oldText, newText }]` 多组替换，每组独立匹配原始文件（非增量）
- 每组 `oldText` 必须在文件中唯一出现且不重叠
- BOM（UTF-8 BOM）剥离，匹配完后重新添加
- 行尾探测（CRLF vs LF）与保留——匹配时归一化为 LF，写入时恢复原始行尾
- 文本模糊匹配：NFKC 正常化、智能单引号/双引号转换、Unicode 破折号归并、特殊空格归并
- Legacy 兼容：如果模型以非数组形式发送 `edits`（如单 `oldText`/`newText` 字段），自动包装为数组
- `edits` JSON string 修复：部分模型将 `edits` 以 JSON string 发送，自动解析
- 生成 diff 供审核（可读 diff string + unified patch）
- 文件变动队列（复用 Write tool 的 `withFileMutationQueue`）
- abort signal 支持

**Blocked by:** None — can start immediately.

- [ ] `EditParams` 类型定义（path: String, edits: Vec<Edit { oldText, newText }>）
- [ ] `edit-diff` 模块：BOM 剥离、行尾探测/归一化/恢复、模糊匹配、applyEdits 逻辑
- [ ] diff 生成：可读 diff string、unified patch、firstChangedLine 定位
- [ ] `prepareArguments` 兼容逻辑：单参数包装、JSON string 修复
- [ ] `EditTool` 实现 `Tool` + `AgentTool` trait
- [ ] 编辑逻辑：access 检查 → 读取 → strip BOM → 行尾归一 → apply edits → restore → 写入 → 返回 diff
- [ ] 注册到 `main.rs`
- [ ] 测试：单组替换、多组替换、CRLF 保留、BOM 保留、模糊匹配、不存在的 oldText（报错）、重叠 edits（报错）、abort
