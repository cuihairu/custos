<script setup lang="ts">
import { computed } from 'vue'
import { useData, withBase } from 'vitepress'

// House-style architecture figure (same pattern as the apollo docs):
// locale-aware SVG from docs/public/images/, caption with a full-size link.
const { lang } = useData()
const english = computed(() => !lang.value.startsWith('zh'))
const language = computed(() => (english.value ? 'en' : 'zh'))
const source = computed(() => withBase(`/images/architecture-${language.value}.svg`))
const description = computed(() =>
  english.value
    ? 'Custos system architecture: CLI, dashboard, tray and Python binding drive a core engine that maintains the resource registry and dependency graph; the recovery engine schedules, executes under policy and budget, and verifies health before declaring recovery; plugin capabilities come from external processes over JSON-RPC stdio; platform adapters carry out OS actions; events persist to the SQLite recovery store and feed the notification engine (Telegram, SMTP, webhook, local log). Solid lines are data and business requests, dashed lines are control and supervision.'
    : 'Custos 系统架构：CLI、Dashboard、托盘与 Python 绑定驱动核心引擎维护资源注册表与依赖图；恢复引擎负责调度、在策略与预算约束下执行动作并通过健康验证判定恢复成功；插件能力来自 stdio JSON-RPC 上的外部进程；平台适配层执行操作系统动作；事件持久化到 SQLite 恢复存储，并驱动通知引擎（Telegram、SMTP、Webhook、本地日志）。实线为数据与业务请求，虚线为控制与监督。',
)
</script>

<template>
  <figure class="architecture-figure" :aria-label="description">
    <div class="architecture-canvas">
      <img :src="source" width="1280" height="1400" :alt="description" decoding="async" />
    </div>
    <figcaption class="architecture-caption">
      <span>{{ english ? 'Custos system architecture. Solid: data / requests, dashed: control / supervision.' : 'Custos 系统架构图。实线：数据 / 请求，虚线：控制 / 监督。' }}</span>
      <a :href="source" target="_blank" rel="noopener noreferrer">
        {{ english ? 'Open full-size diagram' : '查看矢量原图' }}
        <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M7 17 17 7M7 7h10v10" /></svg>
      </a>
    </figcaption>
  </figure>
</template>

<style scoped>
.architecture-figure {
  margin: 28px 0 36px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  overflow: hidden;
}
.architecture-canvas {
  background: #f8fafc;
  padding: 12px;
}
.architecture-canvas img {
  display: block;
  width: 100%;
  height: auto;
  margin: 0;
}
.architecture-caption {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 8px 20px;
  padding: 13px 18px;
  border-top: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.6;
}
.architecture-caption a {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-height: 28px;
  color: var(--vp-c-brand-1);
  font-weight: 500;
  text-decoration: underline;
  text-underline-offset: 3px;
}
.architecture-caption a:focus-visible {
  outline: 2px solid var(--vp-c-brand-1);
  outline-offset: 4px;
  border-radius: 2px;
}
</style>
