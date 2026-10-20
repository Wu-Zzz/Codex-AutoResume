<script setup lang="ts">
import { formatTime, stateLabel } from '../api'
import type { TaskView } from '../types'
import folder from '../assets/figma/workbench-IconFolder1.svg'
import alert from '../assets/figma/workbench-IconAlert.svg'
import chevron from '../assets/figma/workbench-IconChevron.svg'
import grayDot from '../assets/figma/workbench-StatusDot5.svg'

defineProps<{ task: TaskView; busy: boolean; pausedAll: boolean; checkedAt: number | null }>()
defineEmits<{ remove: [] }>()
</script>

<template>
  <section class="task-inspector" aria-label="任务详情">
    <div class="inspector-heading">
      <div class="inspector-title"><h2 :title="task.title">{{ task.title || '未命名任务' }}</h2><button class="text-button remove-button" :disabled="busy" title="仅移出监测列表，不删除原会话" @click="$emit('remove')">移出列表</button></div>
      <p class="inspector-path"><img :src="folder" alt=""/><span :title="task.cwd">{{ task.cwd || '未指定项目' }}</span></p>
    </div>
    <div class="state-panel" :class="task.state">
      <h3><img v-if="task.state === 'needs_attention'" :src="alert" alt=""/><span v-else class="state-orb" aria-hidden="true"/>{{ stateLabel(task.state) }}</h3>
      <p>{{ task.detail || '暂无更多状态信息。' }}</p>
      <p v-if="task.state === 'waiting_quota'" class="quota-time">{{ task.resetsAt === null ? '额度重置时间尚不确定' : `${formatTime(task.resetsAt, true)} 后核对额度` }}</p>
    </div>
    <details class="task-diagnostics" :key="task.id">
      <summary><img :src="chevron" alt=""/>检查详情<span>{{ pausedAll ? '全局已暂停' : task.enabled ? '监测已开启' : '监测已关闭' }}</span></summary>
      <dl>
        <div><dt>会话</dt><dd class="mono selectable">{{ task.id }}</dd></div>
        <div><dt>最近核对</dt><dd class="mono">{{ formatTime(checkedAt, true) }}</dd></div>
        <div><dt>自动续跑</dt><dd>尚未开放</dd></div>
      </dl>
    </details>
    <div class="activity">
      <div class="activity-heading"><h3>活动记录</h3><span>最近 {{ Math.min(task.events.length, 6) }} 条</span></div>
      <ol v-if="task.events.length" class="activity-list">
        <li v-for="(event, index) in task.events.slice(-6).reverse()" :key="`${event.at}-${index}`"><time>{{ formatTime(event.at, true) }}</time><img :src="grayDot" alt=""/><span>{{ event.message }}</span></li>
      </ol>
      <p v-else class="activity-empty">暂无记录</p>
    </div>
  </section>
</template>
