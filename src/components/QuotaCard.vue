<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { formatTime } from '../api'
import { quotaCountdown, quotaLabel, validPercent } from '../quota'
import type { QuotaTelemetry, QuotaWindow } from '../types'

const props = defineProps<{ quota?: QuotaTelemetry | null }>()
const selectedId = ref('')
const now = ref(Date.now() / 1000)
const timer = setInterval(() => { now.value = Date.now() / 1000 }, 1000)
onBeforeUnmount(() => clearInterval(timer))
const buckets = computed(() => props.quota?.buckets ?? [])
watch(buckets, value => {
  if (!value.some(bucket => bucket.id === selectedId.value)) selectedId.value = value[0]?.id ?? ''
}, { immediate: true })
const selected = computed(() => buckets.value.find(bucket => bucket.id === selectedId.value))
const blockedCount = computed(() => buckets.value.filter(bucket => bucket.blocked).length)
const stale = computed(() => !!props.quota && now.value - props.quota.observedAt > 60)
const windows = computed<QuotaWindow[]>(() => selected.value?.windows.length ? selected.value.windows : [
  { kind: 'primary', durationMins: null, usedPercent: null, resetsAt: null },
  { kind: 'secondary', durationMins: null, usedPercent: null, resetsAt: null },
])
const source = computed(() => props.quota?.source === 'desktop' ? '原聊天账户' : props.quota?.source === 'managed' ? '终端账户' : '监测账户')
</script>

<template>
  <section class="quota-card" aria-label="账户额度">
    <div class="quota-card-heading">
      <span>{{ source }}</span>
      <label v-if="buckets.length > 1" class="quota-bucket">
        <span class="sr-only">选择额度类型</span>
        <select v-model="selectedId"><option v-for="bucket in buckets" :key="bucket.id" :value="bucket.id">{{ bucket.label || bucket.id }}</option></select>
      </label>
      <span v-else-if="selected?.label" class="quota-bucket-label" :title="selected.label">{{ selected.label }}</span>
      <span v-if="stale" class="quota-freshness" title="显示上次读取的额度，续跑前仍会重新核对">待更新</span>
    </div>
    <div class="quota-windows">
      <div v-for="window in windows" :key="window.kind" class="quota-window" :class="{ exhausted: validPercent(window.usedPercent) === 100, near: (validPercent(window.usedPercent) ?? 0) >= 95 && window.usedPercent !== 100, unknown: validPercent(window.usedPercent) === null }">
        <div class="quota-label-row"><span>{{ quotaLabel(window) }}</span><strong class="mono">{{ validPercent(window.usedPercent) === null ? '--%' : `${Number(window.usedPercent?.toFixed(1))}%` }}</strong></div>
        <div class="quota-bar" :class="{ uncertain: validPercent(window.usedPercent) === 100 && window.resetsAt === null }" role="progressbar" :aria-label="`${quotaLabel(window)}已使用`" :aria-valuenow="validPercent(window.usedPercent) ?? undefined" :aria-valuetext="validPercent(window.usedPercent) === null ? '使用比例未知' : `已使用 ${window.usedPercent}%`" aria-valuemin="0" aria-valuemax="100"><span v-if="validPercent(window.usedPercent) !== null" :style="{ width: `${window.usedPercent}%` }"/></div>
        <div class="quota-use">{{ validPercent(window.usedPercent) === null ? '使用比例未知' : window.usedPercent === 100 ? '额度已用尽' : '已使用' }}</div>
        <div class="quota-reset mono" :title="window.resetsAt === null ? undefined : `重置时间 ${formatTime(window.resetsAt, true)}`">{{ quotaCountdown(window.resetsAt, now) }}</div>
      </div>
    </div>
    <div v-if="!quota || !buckets.length" class="quota-footnote">尚未读取到额度</div>
    <div v-else-if="buckets.length > 1 && blockedCount" class="quota-footnote">{{ blockedCount }} 项额度受限</div>
  </section>
</template>
