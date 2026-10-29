---
layout: home

hero:
  name: Custos
  text: 跨平台本地自愈引擎
  tagline: 资源与依赖图驱动的故障检测、诊断与自动恢复 —— Rust 实现，本地自治优先，恢复成功必须通过健康验证
  actions:
    - theme: brand
      text: 开始阅读
      link: /guide/overview
    - theme: alt
      text: 分阶段实施计划
      link: /guide/roadmap
    - theme: alt
      text: 实施任务书
      link: /guide/agent-brief

features:
  - title: 依赖图驱动恢复
    details: 每个资源具备检测、诊断、恢复与验证能力；依赖未满足时先按策略恢复依赖。
  - title: 插件化能力封装
    details: 插件提供能力，Core Engine 统一负责依赖协调、执行权限、恢复预算、审计与最终判定。
  - title: 本地自治优先
    details: 网络断开时仍能检测、执行修复、持久化记录；恢复后重试外发通知。
  - title: 健康验证口径
    details: 恢复成功必须通过健康验证，不能仅根据命令退出码判定。
---
