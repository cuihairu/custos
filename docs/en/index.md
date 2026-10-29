---
layout: home

hero:
  name: Custos
  text: Cross-platform local self-healing engine
  tagline: Dependency-graph-driven failure detection, diagnosis and automatic recovery — implemented in Rust, local autonomy first, and a recovery only counts as successful when it passes health verification
  actions:
    - theme: brand
      text: Start reading
      link: /en/guide/overview
    - theme: alt
      text: Phased implementation plan
      link: /en/guide/roadmap
    - theme: alt
      text: Implementation brief
      link: /en/guide/agent-brief

features:
  - title: Dependency-graph-driven recovery
    details: Every resource can probe, diagnose, recover and verify; when dependencies are unmet, dependencies are recovered first according to policy.
  - title: Plugin-based capability encapsulation
    details: Plugins provide capabilities, while the Core Engine uniformly handles dependency coordination, execution permissions, recovery budgets, auditing and final verdicts.
  - title: Local autonomy first
    details: Detection, repair execution and persisted records keep working while the network is down; outbound notifications are retried after recovery.
  - title: Health verification rule
    details: A recovery only counts as successful when it passes health verification — never based on a command exit code alone.
---
