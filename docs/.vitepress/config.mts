import { defineConfig } from 'vitepress'

const enSidebar = {
  text: 'Design Documents',
  items: [
    { text: 'Overview and Architecture Decisions', link: '/guide/overview' },
    { text: 'Resource and Dependency Model', link: '/guide/resources' },
    { text: 'Plugin Architecture', link: '/guide/plugins' },
    { text: 'Probing, Diagnosis and Failure Verdicts', link: '/guide/probe-diagnosis' },
    { text: 'Probe Survey and Gap Analysis', link: '/guide/probe-survey' },
    { text: 'Recovery Engine', link: '/guide/recovery-engine' },
    { text: 'Persistence and Crash Consistency', link: '/guide/persistence' },
    { text: 'Notification Engine', link: '/guide/notification' },
    { text: 'Dashboard, CLI and Configuration', link: '/guide/dashboard-cli-config' },
    { text: 'Cross-platform Architecture', link: '/guide/platforms' },
    { text: 'Security and Permissions', link: '/guide/security' },
    { text: 'Cargo Workspace', link: '/guide/workspace' },
    { text: 'Phased Implementation Plan', link: '/guide/roadmap' },
    { text: 'Code Agent Protocol', link: '/guide/agent-protocol' },
    { text: 'Testing and Acceptance', link: '/guide/testing' },
    { text: 'ADRs and Implementation Principles', link: '/guide/adr-principles' },
    { text: 'Implementation Brief', link: '/guide/agent-brief' },
  ],
}

const enAdrSidebar = {
  text: 'Architecture Decision Records',
  items: [
    { text: 'Index', link: '/adr/index' },
    { text: '0001 Why Rust', link: '/adr/ADR-0001-why-rust' },
    { text: '0002 Why External-process Plugins', link: '/adr/ADR-0002-why-external-process-plugins' },
    { text: '0003 Why JSON-RPC over stdio', link: '/adr/ADR-0003-why-jsonrpc-over-stdio' },
    { text: '0004 Why Resource + Dependency Graph', link: '/adr/ADR-0004-why-resource-dependency-graph' },
    { text: '0005 Why the Core Coordinates the Graph', link: '/adr/ADR-0005-why-core-coordinates-the-graph' },
    { text: '0006 Why Independent Verification', link: '/adr/ADR-0006-why-independent-verification' },
    { text: '0007 Why SQLite', link: '/adr/ADR-0007-why-sqlite' },
    { text: '0008 Why Persist Before High-risk Actions', link: '/adr/ADR-0008-why-persist-before-high-risk-actions' },
    { text: '0009 Why Notification is Decoupled', link: '/adr/ADR-0009-why-notification-decoupled' },
    { text: '0010 Why Dashboard is Off by Default', link: '/adr/ADR-0010-why-dashboard-off-by-default' },
    { text: '0011 Why P0 is Process/Exec Only', link: '/adr/ADR-0011-why-p0-only-process-exec-probes' },
    { text: '0012 Why No Unauthorized High-risk Network Actions', link: '/adr/ADR-0012-why-unauthorized-high-risk-network-actions-are-forbidden' },
    { text: '0013 Why Unknown Outcomes Are Never Retried', link: '/adr/ADR-0013-why-unknown-outcomes-are-never-blindly-retried' },
    { text: '0014 Why No Dependency Cycles', link: '/adr/ADR-0014-why-ordinary-dependency-cycles-are-forbidden' },
    { text: '0015 Internal Toolbox Model', link: '/adr/ADR-0015-internal-toolbox-plugin-rule-layer' },
  ],
}

const zhSidebar = {
  text: '设计文档',
  items: [
    { text: '概述与架构决策', link: '/zh/guide/overview' },
    { text: 'Resource 与 Dependency 模型', link: '/zh/guide/resources' },
    { text: '插件架构', link: '/zh/guide/plugins' },
    { text: '探测、诊断与故障判定', link: '/zh/guide/probe-diagnosis' },
    { text: '探测类型调研与差距分析', link: '/zh/guide/probe-survey' },
    { text: 'Recovery Engine', link: '/zh/guide/recovery-engine' },
    { text: '持久化与崩溃一致性', link: '/zh/guide/persistence' },
    { text: 'Notification Engine', link: '/zh/guide/notification' },
    { text: 'Dashboard、CLI 与配置', link: '/zh/guide/dashboard-cli-config' },
    { text: '跨平台架构', link: '/zh/guide/platforms' },
    { text: '安全与权限', link: '/zh/guide/security' },
    { text: 'Cargo Workspace', link: '/zh/guide/workspace' },
    { text: '分阶段实施计划', link: '/zh/guide/roadmap' },
    { text: 'Code Agent 执行规范', link: '/zh/guide/agent-protocol' },
    { text: '测试与验收体系', link: '/zh/guide/testing' },
    { text: 'ADR 与最终实施原则', link: '/zh/guide/adr-principles' },
    { text: '实施任务书', link: '/zh/guide/agent-brief' },
  ],
}

const zhAdrSidebar = {
  text: '架构决策记录（ADR）',
  items: [
    { text: '索引', link: '/zh/adr/index' },
    { text: '0001 为什么选择 Rust', link: '/zh/adr/ADR-0001-why-rust' },
    { text: '0002 为什么外部进程插件', link: '/zh/adr/ADR-0002-why-external-process-plugins' },
    { text: '0003 为什么 JSON-RPC over stdio', link: '/zh/adr/ADR-0003-why-jsonrpc-over-stdio' },
    { text: '0004 为什么 Resource + 依赖图', link: '/zh/adr/ADR-0004-why-resource-dependency-graph' },
    { text: '0005 为什么核心协调依赖图', link: '/zh/adr/ADR-0005-why-core-coordinates-the-graph' },
    { text: '0006 为什么独立验证', link: '/zh/adr/ADR-0006-why-independent-verification' },
    { text: '0007 为什么 SQLite', link: '/zh/adr/ADR-0007-why-sqlite' },
    { text: '0008 为什么高风险前持久化', link: '/zh/adr/ADR-0008-why-persist-before-high-risk-actions' },
    { text: '0009 为什么通知解耦', link: '/zh/adr/ADR-0009-why-notification-decoupled' },
    { text: '0010 为什么 Dashboard 默认关闭', link: '/zh/adr/ADR-0010-why-dashboard-off-by-default' },
    { text: '0011 为什么 P0 只有 Process/Exec', link: '/zh/adr/ADR-0011-why-p0-only-process-exec-probes' },
    { text: '0012 为什么禁止未授权高风险', link: '/zh/adr/ADR-0012-why-unauthorized-high-risk-network-actions-are-forbidden' },
    { text: '0013 为什么未知不盲重试', link: '/zh/adr/ADR-0013-why-unknown-outcomes-are-never-blindly-retried' },
    { text: '0014 为什么禁止依赖环', link: '/zh/adr/ADR-0014-why-ordinary-dependency-cycles-are-forbidden' },
    { text: '0015 内部工具箱模型', link: '/zh/adr/ADR-0015-internal-toolbox-plugin-rule-layer' },
  ],
}

export default defineConfig({
  locales: {
    root: {
      label: 'English',
      lang: 'en',
      title: 'Custos',
      description:
        'Cross-platform, locally autonomous, dependency-aware failure detection and automatic recovery engine',
      themeConfig: {
        siteTitle: 'Custos',
        logo: '/logo.svg',
        outline: [2, 3],
        sidebar: [enSidebar, enAdrSidebar],
        socialLinks: [
          { icon: 'github', link: 'https://github.com/cuihairu/custos' },
        ],
      },
    },
    zh: {
      label: '简体中文',
      lang: 'zh-CN',
      title: 'Custos',
      description: '跨平台、本地自治、依赖感知的故障检测与自动恢复引擎',
      themeConfig: {
        siteTitle: 'Custos',
        logo: '/logo.svg',
        outline: [2, 3],
        sidebar: [zhSidebar, zhAdrSidebar],
        socialLinks: [
          { icon: 'github', link: 'https://github.com/cuihairu/custos' },
        ],
      },
    },
  },
  head: [
    ['meta', { name: 'theme-color', content: '#3eaf7c' }],
  ],
  srcDir: '.',
  outDir: '../.vitepress/dist',
  base: "/custos/",
  cleanUrls: true,
  lastUpdated: true,
})
