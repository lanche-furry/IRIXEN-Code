# 贡献指南

<p align="center">
  中文 &nbsp;|&nbsp; <a href="./CONTRIBUTING.en.md">English</a>
</p>

## 开发环境

```bash
npm install
npm run app:dev      # 热更新的完整应用
npm run check        # 只编译 Rust（快）
npm run build        # 只构建前端 + 类型检查
```

工具链细节见 [docs/BUILD.md](docs/BUILD.md)。

## 代码地图

改哪一块，看这里：

| 想改 | 去哪 |
| --- | --- |
| 新增厂商 / 修协议兼容 | `src-tauri/src/agent/provider.rs`（加一个 `impl LlmProvider`） |
| 思考强度识别规则 | `src-tauri/src/agent/models.rs::detect_thinking` |
| 新增原生工具 | `src-tauri/src/agent/tools.rs`（`native_tool_specs` + `execute_native` 两处） |
| 压缩策略 | `src-tauri/src/agent/context.rs` |
| 系统提示词 / 轮次上限 | `src-tauri/src/config.rs` 的 `AgentConfig::default` |
| 内置技能 | `src-tauri/src/skills/builtin/*.md` |
| 界面布局 | `src/styles/app.css`（分区都是独立 block） |
| 颜色 / 圆角 / 字号 | `src/styles/base.css` 顶部的 `:root` 令牌 |
| 文案 | `src/core/i18n.ts`（5 种语言，键必须对齐） |
| 对话渲染 | `src/ui/chat.ts` |

## 约定

**Rust**

- 命令层（`commands.rs`）只做转发与 `map_err(|e| e.to_string())`，不放业务逻辑
- 需要 `async` + 共享状态时，别让 `MutexGuard` 跨 `await`：先在锁内 clone 出需要的数据，再释放锁
- 用户可见的错误信息用中文，且要说明**怎么修**（"未配置供应商 → 打开 设置 → 供应商"）

**TypeScript**

- 不用框架，`h()` 建节点，状态变化显式重绘
- 类型必须和 Rust 端 `serde` 输出对齐（`src/types.ts` 是唯一镜像）
- 新增文案必须同时补齐 5 个语言字典，缺键会回退英文——但不能故意留坑

**CSS**

- 只用 `base.css` 里定义的变量，不写魔法色值（自定义主题靠覆盖变量工作）
- 新样式加到 `app.css` 对应分区，保持文件里"标题栏 → 活动栏 → 侧栏 → 编辑器 → 面板 → 对话 → 状态栏 → 设置"的顺序

## 提交

提交信息用一句话说清"改了什么、为什么"：

```
修复 Gemini 流式响应中 usage 丢失

- 解析 usageMetadata 时同时读 promptTokenCount / candidatesTokenCount
- 之前只在最后一个 chunk 里读，部分中转站会分片下发
```

## Pull Request

1. 说明动机（解决什么问题 / 补什么能力）
2. UI 改动附截图或 GIF
3. 涉协议改动说明测过的厂商与模型
4. 保持 `npm run build` 与 `npm run check` 通过

## 许可

贡献即同意以 [Apache License 2.0](LICENSE) 授权。
