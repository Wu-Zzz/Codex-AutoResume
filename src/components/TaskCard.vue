<script setup lang="ts">
import { computed } from 'vue'
import { formatTime, projectName, stateLabel } from '../api'
import type { TaskView } from '../types'
import folder from '../assets/figma/workbench-IconFolder.svg'
import switchOn from '../assets/figma/workbench-DarkSwitchOn.svg'
import redDot from '../assets/figma/workbench-StatusDot.svg'
import amberDot from '../assets/figma/workbench-StatusDot1.svg'
import greenDot from '../assets/figma/workbench-StatusDot2.svg'
import blueDot from '../assets/figma/workbench-StatusDot4.svg'
import grayDot from '../assets/figma/workbench-StatusDot5.svg'

const props = defineProps<{ task: TaskView; selected?: boolean; busy?: boolean; pausedAll: boolean }>()
defineEmits<{ select: []; toggle: [] }>()
const dot = computed(() => ({
  needs_attention: redDot, waiting_quota: amberDot, running: greenDot,
  ready: greenDot, monitoring: greenDot, checking: blueDot, reconciling: blueDot,
}[props.task.state] ?? grayDot))
const hint = computed(() => {
  if (props.pausedAll) return '全局已暂停'
  if (!props.task.enabled) return props.task.mode !== 'monitor' ? '续跑已关闭' : '监测已关闭'
  if (props.task.state === 'waiting_quota') return props.task.resetsAt === null ? '重置时间未知' : `${formatTime(props.task.resetsAt, true)} 后核对`
  return ''
})
</script>

<template>
  <article class="task-card" :class="[task.state, { selected }]" :data-task-id="task.id">
    <button class="task-card-content" :aria-label="`查看${task.title || '未命名任务'}详情`" :aria-pressed="selected" @click="$emit('select')">
      <strong class="task-name" :title="task.title">{{ task.title || '未命名任务' }}</strong>
      <span class="task-project"><img :src="folder" alt=""/><span :title="task.cwd">{{ projectName(task.cwd) }}</span><span class="task-mode">{{ task.mode === 'desktop' ? '原聊天 · 实验' : task.mode === 'managed' ? '终端托管' : '仅监测' }}</span></span>
      <span class="task-state-row">
        <span class="status-label" :class="task.state"><img :src="dot" alt=""/>{{ stateLabel(task.state, task.mode) }}</span>
        <span class="task-hint" :title="hint">{{ hint }}</span>
      </span>
    </button>
    <button class="switch" :class="{ on: task.enabled }" role="switch" :aria-checked="task.enabled" :aria-label="`${task.title}${task.mode !== 'monitor' ? '自动续跑' : '监测'}`" aria-describedby="capability-status" :title="task.canEnable === false ? task.detail : `${task.enabled ? '关闭' : '开启'}${task.mode !== 'monitor' ? '自动续跑' : '任务监测'}`" :disabled="busy || (!task.enabled && task.canEnable === false)" @click="$emit('toggle')">
      <img v-if="task.enabled" :src="switchOn" alt=""/>
      <span v-else class="switch-off" aria-hidden="true"><span/></span>
    </button>
  </article>
</template>
