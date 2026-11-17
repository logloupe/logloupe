<script setup lang="ts">
// The server a site's logs are fetched from over SSH: set it up, see what is
// on it, and ask for a fetch. The Ingest screen runs the fetch itself so that
// progress and results look the same as for local files.
import { reactive, ref, watch } from 'vue'
import { api, errorMessage, type Remote, type RemoteFile, type RemoteListing } from '../api'
import { state } from '../state'
import { formatBytes, formatUnix } from '../format'

const props = defineProps<{ running: boolean }>()
const emit = defineEmits<{ fetch: [days: number | null] }>()
/** Shared with the file ingest: do not refresh the bot IP ranges first. */
const offline = defineModel<boolean>('offline', { default: false })
const toast = useToast()

const remote = ref<Remote | null>(null)
const loaded = ref(false)
const editing = ref(false)
const saving = ref(false)
const formError = ref('')
const form = reactive({ host: '', port: '', paths: '' })

const days = ref('')
const checking = ref(false)
const listing = ref<RemoteListing | null>(null)
const listError = ref('')

const stateLabel: Record<RemoteFile['state'], string> = {
  new: 'New',
  changed: 'Changed',
  fetched: 'Already fetched',
  old: 'Too old',
}
const stateColour: Record<RemoteFile['state'], 'success' | 'info' | 'neutral'> = {
  new: 'success',
  changed: 'info',
  fetched: 'neutral',
  old: 'neutral',
}

function dayLimit(): number | null {
  const n = Number(days.value)
  return days.value !== '' && Number.isInteger(n) && n > 0 ? n : null
}

async function load() {
  loaded.value = false
  editing.value = false
  listing.value = null
  listError.value = ''
  remote.value = null
  if (!state.slug) return
  try {
    remote.value = await api.getRemote(state.slug)
    loaded.value = true
  } catch {
    // The site is in use (an ingest is running). The card stays hidden rather
    // than offering to set up a server that may already be set; it is loaded
    // again when the run ends.
  }
}

function openForm() {
  Object.assign(form, {
    host: remote.value?.host ?? '',
    port: remote.value?.port ? String(remote.value.port) : '',
    paths: remote.value?.paths.join('\n') ?? '',
  })
  formError.value = ''
  editing.value = true
}

async function save() {
  saving.value = true
  formError.value = ''
  try {
    remote.value = await api.setRemote(state.slug, {
      host: form.host,
      port: form.port ? Number(form.port) : null,
      paths: form.paths.split('\n'),
    })
    editing.value = false
    listing.value = null
  } catch (e) {
    formError.value = errorMessage(e)
  } finally {
    saving.value = false
  }
}

async function forget() {
  try {
    await api.removeRemote(state.slug)
    toast.add({ title: 'Server removed', description: 'Logs already fetched are kept.', color: 'success' })
    await load()
  } catch (e) {
    formError.value = errorMessage(e)
  }
}

async function check() {
  checking.value = true
  listError.value = ''
  try {
    listing.value = await api.listRemote(state.slug, dayLimit())
  } catch (e) {
    listing.value = null
    listError.value = errorMessage(e)
  } finally {
    checking.value = false
  }
}

watch(() => state.slug, load, { immediate: true })
// A fetch changes what counts as new, so the list shown before it is stale.
watch(
  () => props.running,
  (running) => {
    if (running) return
    listing.value = null
    if (!loaded.value) load()
  },
)
</script>

<template>
  <section v-if="loaded" class="rounded-lg border border-default bg-default">
    <header class="flex items-center justify-between gap-4 border-b border-default px-4 py-2.5">
      <h2 class="text-sm font-semibold text-highlighted">Fetch from a server</h2>
      <UButton
        v-if="remote && !editing"
        label="Change"
        size="xs"
        color="neutral"
        variant="ghost"
        :disabled="running"
        @click="openForm"
      />
    </header>

    <div v-if="!remote && !editing" class="flex flex-wrap items-center justify-between gap-4 px-4 py-3">
      <p class="text-sm text-muted">
        Logloupe can copy new logs from your server over SSH and ingest them, so you do not have to download them first.
      </p>
      <UButton label="Set up a server" color="neutral" variant="outline" @click="openForm" />
    </div>

    <form v-else-if="editing" class="space-y-4 px-4 py-3" @submit.prevent="save">
      <p class="text-sm text-muted">
        Logloupe runs your computer's own <code>ssh</code> program, with your SSH config and keys. It stores no keys or
        passwords and never asks for one, so <code>ssh &lt;server&gt;</code> must already work in a terminal without
        asking for anything.
      </p>
      <div class="grid grid-cols-3 gap-4">
        <UFormField class="col-span-2" label="Server" help="A host alias from your SSH config, or user@host.">
          <UInput v-model="form.host" placeholder="web1" class="w-full" required />
        </UFormField>
        <UFormField label="Port" help="Leave empty for the usual one.">
          <UInput v-model="form.port" type="number" min="1" max="65535" placeholder="22" class="w-full" />
        </UFormField>
      </div>
      <UFormField
        label="Log paths on the server"
        help="One per line. Use * to match rotated files, for example /var/log/nginx/access.log* or ~/logs/example.com-*.gz"
      >
        <UTextarea v-model="form.paths" :rows="3" placeholder="/var/log/nginx/access.log*" class="w-full font-mono" required />
      </UFormField>
      <UAlert v-if="formError" color="error" variant="subtle" :title="formError" />
      <div class="flex items-center justify-between gap-2">
        <UButton v-if="remote" label="Remove server" color="error" variant="ghost" @click="forget" />
        <span v-else />
        <div class="flex gap-2">
          <UButton label="Cancel" color="neutral" variant="ghost" @click="editing = false" />
          <UButton type="submit" label="Save" :loading="saving" />
        </div>
      </div>
    </form>

    <template v-else-if="remote">
      <div class="px-4 py-3 text-sm">
        <p class="font-medium text-highlighted">{{ remote.host }}<template v-if="remote.port">, port {{ remote.port }}</template></p>
        <p v-for="path in remote.paths" :key="path" class="truncate font-mono text-xs text-muted" :title="path">{{ path }}</p>
      </div>
      <footer class="flex flex-wrap items-center justify-between gap-4 border-t border-default px-4 py-3">
        <div class="flex flex-wrap items-center gap-x-6 gap-y-2">
          <label class="flex items-center gap-2 text-sm text-muted">
            Only files changed in the last
            <UInput v-model="days" type="number" min="1" placeholder="any" class="w-20" :disabled="running" />
            days
          </label>
          <USwitch v-model="offline" label="Do not refresh bot IP ranges first" :disabled="running" />
        </div>
        <div class="flex gap-2">
          <UButton label="Check server" color="neutral" variant="outline" :loading="checking" :disabled="running" @click="check" />
          <UButton icon="i-lucide-cloud-download" label="Fetch new files" :loading="running" :disabled="checking" @click="emit('fetch', dayLimit())" />
        </div>
      </footer>

      <UAlert
        v-if="listError"
        class="mx-4 mb-3"
        color="error"
        variant="subtle"
        icon="i-lucide-triangle-alert"
        title="Could not list the server's files"
        :description="listError"
      />
      <div v-if="listing" class="border-t border-default">
        <p v-if="listing.files.length === 0" class="px-4 py-3 text-sm text-muted">No files on {{ listing.host }} match.</p>
        <div v-else class="max-h-64 divide-y divide-default overflow-auto">
          <div v-for="file in listing.files" :key="file.path" class="flex items-center justify-between gap-4 px-4 py-2 text-sm">
            <span class="min-w-0 truncate font-mono text-xs text-highlighted" :title="file.path">{{ file.path }}</span>
            <span class="flex shrink-0 items-center gap-3 tabular-nums text-muted">
              {{ formatBytes(file.size_bytes) }} · {{ formatUnix(file.modified) }}
              <UBadge :label="stateLabel[file.state]" :color="stateColour[file.state]" variant="subtle" size="sm" />
            </span>
          </div>
        </div>
        <p v-for="note in listing.notes" :key="note" class="border-t border-default px-4 py-2 text-sm text-muted">{{ note }}</p>
      </div>
    </template>
  </section>
</template>
