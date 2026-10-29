import { defineConfig } from 'vitepress'

export default defineConfig({
  lang: 'zh-CN',
  title: 'Custos',
  description: '跨平台、本地自治、依赖感知的故障检测与自动恢复引擎',
  themeConfig: {
    siteTitle: 'Custos',
    logo: '/logo.svg',
    outline: [2, 3],
    sidebar: [
      {
        text: '设计文档',
        items: [
          { text: '概述与架构决策', link: '/guide/overview' },
          { text: 'Resource 与 Dependency 模型', link: '/guide/resources' },
          { text: '插件架构', link: '/guide/plugins' },
          { text: '探测、诊断与故障判定', link: '/guide/probe-diagnosis' },
          { text: 'Recovery Engine', link: '/guide/recovery-engine' },
          { text: '持久化与崩溃一致性', link: '/guide/persistence' },
          { text: 'Notification Engine', link: '/guide/notification' },
          { text: 'Dashboard、CLI 与配置', link: '/guide/dashboard-cli-config' },
          { text: '跨平台架构', link: '/guide/platforms' },
          { text: '安全与权限', link: '/guide/security' },
          { text: 'Cargo Workspace', link: '/guide/workspace' },
          { text: '分阶段实施计划', link: '/guide/roadmap' },
          { text: 'Code Agent 执行规范', link: '/guide/agent-protocol' },
          { text: '测试与验收体系', link: '/guide/testing' },
          { text: 'ADR 与最终实施原则', link: '/guide/adr-principles' },
          { text: '实施任务书', link: '/guide/agent-brief' },
        ],
      },
    ],
    socialLinks: [
      { icon: 'github', link: 'https://github.com/cuihairu/custos' },
    ],
  },
})
