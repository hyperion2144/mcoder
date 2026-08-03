// mcoder UI Redesign v2 - PlanApproval（全屏审批视图）
// 对应 prototype: mcoder-ui-redesign/pages/tui-plan-approval.html
// 布局: top-bar (连接/路径/分支 + 键位提示) + 滚动内容
//       (plan-header / steps / affected-files / risks) + bottom-dock (操作栏)
// 卡片用 round border；top-bar / bottom-dock 用 single border。
// 颜色映射: blue=#7aa2f7 green=#9ece6a red=#f7768e purple=#bb9af7
//          orange=#ff9e64 cyan=#7dcfff muted=#565f89

import { Box, Text, useInput } from 'ink';
import { useSessionStore, useUiStore } from '../store/index.js';
import type { WsClient } from '../rpc/client.js';
import { TUI_COLORS, PREFIX } from '../theme.js';

interface Props {
  client: WsClient;
}

type StepKind = 'done' | 'current' | 'pending' | 'failed';

function stepKind(status: string | undefined): StepKind {
  const s = (status || '').toLowerCase();
  if (s === 'done' || s === 'completed') return 'done';
  if (s === 'in_progress' || s === 'current' || s === 'running') return 'current';
  if (s === 'failed' || s === 'error') return 'failed';
  return 'pending';
}

function MetaItem({ k, v, vColor }: { k: string; v: string; vColor?: string }) {
  return (
    <Text>
      <Text color={TUI_COLORS.textMuted}>{k} </Text>
      <Text color={vColor || TUI_COLORS.textPrimary}>{v}</Text>
    </Text>
  );
}

export function PlanApproval({ client }: Props) {
  const {
    pendingPlan,
    currentSessionId,
    setPendingPlan,
    projectPath,
    gitBranch,
    connected,
    currentModel,
  } = useSessionStore();
  const setView = useUiStore((s) => s.setView);

  useInput((input: string, key: any) => {
    if (!pendingPlan) return;
    const sid = currentSessionId;
    if (!sid) return;
    const k = input.toLowerCase();

    if (k === 'y') {
      client
        .request('session.approve', { session_id: sid, plan_id: pendingPlan.id || '' })
        .then(() => setPendingPlan(null))
        .catch(() => {});
    } else if (k === 'n') {
      client
        .request('session.approve', { session_id: sid, plan_id: pendingPlan.id || '', action: 'reject' })
        .then(() => setPendingPlan(null))
        .catch(() => {});
    } else if (k === 'e') {
      setPendingPlan(null);
    } else if (key.escape) {
      setPendingPlan(null);
      setView('chat');
    }
  });

  if (!pendingPlan) return null;

  const plan: any = pendingPlan;
  const steps: any[] = Array.isArray(plan.steps) ? plan.steps : [];

  // ---- 派生 meta（防御式：真实 plan 结构仅含 steps / created_at）----
  const title =
    plan.title || plan.name || plan.goal ||
    (steps[0] && (steps[0].description || steps[0].text)) || 'Plan';
  const created = plan.created_at || plan.created || plan.createdAt || '-';
  const planModel = plan.model || plan.plan_model || plan.planModel || currentModel || '-';
  const estTurns =
    plan.estimated_turns ?? plan.estimatedTurns ?? plan.turns ?? (steps.length || '-');

  // affected files: 跨 steps 去重 + plan.affected_files
  const fileSet = new Set<string>();
  for (const s of steps) {
    const fa = s.files_affected || s.files || s.filesAffected;
    if (Array.isArray(fa)) for (const f of fa) if (typeof f === 'string') fileSet.add(f);
  }
  const aff = plan.affected_files || plan.affectedFiles;
  if (Array.isArray(aff)) {
    for (const f of aff) {
      if (typeof f === 'string') fileSet.add(f);
      else if (f && typeof f === 'object' && typeof f.path === 'string') fileSet.add(f.path);
    }
  }
  const affectedFiles = Array.from(fileSet);
  const estFiles =
    plan.estimated_files ?? plan.estimatedFiles ?? (affectedFiles.length || '-');
  const estCost = plan.estimated_cost ?? plan.estimatedCost ?? plan.cost ?? '-';
  const complexity = plan.complexity || '-';

  const doneCount = steps.filter((s) => stepKind(s.status) === 'done').length;
  const currentCount = steps.filter((s) => stepKind(s.status) === 'current').length;

  const risks: any[] = Array.isArray(plan.risks) ? plan.risks : [];

  return (
    <Box flexDirection="column" flexGrow={1} overflow="hidden">
      {/* ===== Top bar ===== */}
      <Box flexDirection="row" borderStyle="single" borderColor={TUI_COLORS.textMuted} paddingX={1} flexShrink={0}>
        <Box flexDirection="row" flexGrow={1} gap={1}>
          <Text color={connected ? TUI_COLORS.success : TUI_COLORS.error}>
            {connected ? PREFIX.dot : PREFIX.open}
          </Text>
          <Text color={TUI_COLORS.textPrimary}>{connected ? 'connected' : 'disconnected'}</Text>
          <Text color={TUI_COLORS.textMuted}>{PREFIX.sep}</Text>
          <Text color={TUI_COLORS.cyan}>{projectPath || '~'}</Text>
          <Text color={TUI_COLORS.textMuted}>{PREFIX.sep}</Text>
          <Text color={TUI_COLORS.mauve}>{gitBranch || '-'}</Text>
        </Box>
        <Box flexDirection="row" gap={2} flexShrink={0}>
          <Text><Text color={TUI_COLORS.brand}>[Esc]</Text> <Text color={TUI_COLORS.textMuted}>cancel</Text></Text>
          <Text><Text color={TUI_COLORS.brand}>[y]</Text> <Text color={TUI_COLORS.textMuted}>approve</Text></Text>
          <Text><Text color={TUI_COLORS.brand}>[e]</Text> <Text color={TUI_COLORS.textMuted}>edit</Text></Text>
        </Box>
      </Box>

      {/* ===== Scrollable content ===== */}
      <Box flexDirection="column" flexGrow={1} overflow="hidden" paddingX={1} paddingY={1}>

        {/* ---- Plan header card ---- */}
        <Box flexDirection="column" borderStyle="round" borderColor={TUI_COLORS.brand} paddingX={2} paddingY={1} flexShrink={0} marginBottom={1}>
          <Box flexDirection="row" marginBottom={1}>
            <Text bold color={TUI_COLORS.brand}>Plan</Text>
            <Text color={TUI_COLORS.textMuted}> {PREFIX.sep} </Text>
            <Text bold color={TUI_COLORS.brand}>{title}</Text>
          </Box>
          <Box flexDirection="column">
            <Box flexDirection="row">
              <Box flexGrow={1}><MetaItem k="Created" v={created} /></Box>
              <Box flexGrow={1}><MetaItem k="Plan model" v={planModel} vColor={TUI_COLORS.mauve} /></Box>
              <Box flexGrow={1}><MetaItem k="Estimated turns" v={String(estTurns)} /></Box>
            </Box>
            <Box flexDirection="row" marginTop={1}>
              <Box flexGrow={1}><MetaItem k="Estimated files" v={String(estFiles)} /></Box>
              <Box flexGrow={1}><MetaItem k="Estimated cost" v={String(estCost)} vColor={TUI_COLORS.success} /></Box>
              <Box flexGrow={1}><MetaItem k="Complexity" v={String(complexity)} vColor={TUI_COLORS.orange} /></Box>
            </Box>
          </Box>
        </Box>

        {/* ---- Steps card ---- */}
        <Box flexDirection="column" borderStyle="round" borderColor={TUI_COLORS.textMuted} flexShrink={0} marginBottom={1}>
          <Box flexDirection="row" paddingX={1}>
            <Text color={TUI_COLORS.textSecondary} bold>{PREFIX.pending} Steps</Text>
            <Box flexGrow={1} />
            <Text color={TUI_COLORS.textMuted}>
              {`${steps.length} steps${doneCount ? ` · ${doneCount} done` : ''}${currentCount ? ` · ${currentCount} current` : ''}`}
            </Text>
          </Box>
          <Box flexDirection="column" paddingX={1} marginBottom={1} marginTop={1}>
            {steps.length === 0 ? (
              <Text color={TUI_COLORS.textMuted}>(empty plan)</Text>
            ) : (
              steps.map((step: any, i: number) => {
                const kind = stepKind(step.status);
                const marker = kind === 'done' ? PREFIX.done
                  : kind === 'current' ? PREFIX.dot
                  : kind === 'failed' ? PREFIX.failed
                  : PREFIX.open;
                const markerColor = kind === 'done' ? TUI_COLORS.success
                  : kind === 'current' ? TUI_COLORS.brand
                  : kind === 'failed' ? TUI_COLORS.error
                  : TUI_COLORS.textMuted;
                const stepTitle = step.description || step.text || step.title || '(no description)';
                const isCurrent = kind === 'current';
                const subFiles: string[] = Array.isArray(step.files_affected)
                  ? step.files_affected
                  : Array.isArray(step.files) ? step.files : [];
                const note = step.note || step.note_text || null;

                return (
                  <Box key={i} flexDirection="column" marginTop={i === 0 ? 0 : 1}>
                    <Text>
                      <Text color={TUI_COLORS.textMuted}>{`${i + 1}.`}</Text>
                      <Text>{' '}</Text>
                      <Text color={markerColor}>{marker}</Text>
                      <Text>{' '}</Text>
                      <Text color={isCurrent ? TUI_COLORS.brand : TUI_COLORS.textPrimary} bold={isCurrent}>
                        {stepTitle}
                      </Text>
                      {isCurrent ? <Text color={TUI_COLORS.brand}> [current step]</Text> : null}
                    </Text>
                    {(subFiles.length > 0 || note) && (
                      <Box flexDirection="column">
                        {subFiles.map((f, j) => (
                          <Text key={`f${j}`}>
                            <Text color={TUI_COLORS.textMuted}>{'  '}{PREFIX.branch}{' '}</Text>
                            <Text color={TUI_COLORS.cyan}>{f}</Text>
                          </Text>
                        ))}
                        {note ? (
                          <Text>
                            <Text color={TUI_COLORS.textMuted}>{'  '}{PREFIX.branch}{' '}</Text>
                            <Text color={TUI_COLORS.textMuted}>{note}</Text>
                          </Text>
                        ) : null}
                      </Box>
                    )}
                  </Box>
                );
              })
            )}
          </Box>
        </Box>

        {/* ---- Affected files card ---- */}
        <Box flexDirection="column" borderStyle="round" borderColor={TUI_COLORS.textMuted} flexShrink={0} marginBottom={1}>
          <Box flexDirection="row" paddingX={1}>
            <Text color={TUI_COLORS.textSecondary} bold>{PREFIX.pending} Affected Files</Text>
            <Box flexGrow={1} />
            <Text color={TUI_COLORS.textMuted}>{`${affectedFiles.length} files`}</Text>
          </Box>
          <Box flexDirection="column" paddingX={1} marginBottom={1} marginTop={1}>
            {affectedFiles.length === 0 ? (
              <Text color={TUI_COLORS.textMuted}>(no files affected)</Text>
            ) : (
              affectedFiles.map((f, i) => (
                <Box key={i} flexDirection="row">
                  <Text color={TUI_COLORS.cyan}>{f}</Text>
                  <Box flexGrow={1} />
                  <Text color={TUI_COLORS.mauve}>edit</Text>
                </Box>
              ))
            )}
          </Box>
        </Box>

        {/* ---- Risks card ---- */}
        <Box flexDirection="column" borderStyle="round" borderColor={TUI_COLORS.orange} flexShrink={0} marginBottom={1}>
          <Box flexDirection="row" paddingX={1}>
            <Text color={TUI_COLORS.orange} bold>{`${PREFIX.pending} Risks & Considerations`}</Text>
            <Box flexGrow={1} />
            <Text color={TUI_COLORS.textMuted}>{`${risks.length} items`}</Text>
          </Box>
          <Box flexDirection="column" paddingX={1} marginBottom={1} marginTop={1}>
            {risks.length === 0 ? (
              <Text color={TUI_COLORS.textMuted}>(no risks identified)</Text>
            ) : (
              risks.map((r: any, i: number) => {
                const sev = (r.severity || r.level || '').toLowerCase();
                const isBreak = sev.includes('break') || sev.includes('critical') || sev.includes('high');
                const iconColor = isBreak ? TUI_COLORS.error : TUI_COLORS.orange;
                const icon = isBreak ? '!' : '~';
                const tag = r.tag || r.label || (isBreak ? 'Breaking change:' : 'Warning:');
                const text = r.text || r.description || r.message || '';
                const reco = r.recommendation || r.reco || r.recommendation_text || '';
                return (
                  <Box key={i} flexDirection="row" marginTop={i === 0 ? 0 : 1} gap={1}>
                    <Text color={iconColor} bold>{icon}</Text>
                    <Box flexDirection="column">
                      <Text>
                        <Text color={iconColor} bold>{tag}</Text>
                        <Text color={TUI_COLORS.textPrimary}>{` ${text}`}</Text>
                      </Text>
                      {reco ? (
                        <Text color={TUI_COLORS.textMuted}>
                          <Text color={TUI_COLORS.cyan}>{'->'}</Text>
                          {` ${reco}`}
                        </Text>
                      ) : null}
                    </Box>
                  </Box>
                );
              })
            )}
          </Box>
        </Box>
      </Box>

      {/* ===== Bottom dock ===== */}
      <Box flexDirection="column" borderStyle="single" borderColor={TUI_COLORS.textMuted} paddingX={1} flexShrink={0}>
        <Box flexDirection="row" gap={2}>
          <Text>
            <Text color={TUI_COLORS.brand} bold>[y]</Text>
            <Text color={TUI_COLORS.textPrimary}>{' approve & execute'}</Text>
          </Text>
          <Text>
            <Text color={TUI_COLORS.brand} bold>[e]</Text>
            <Text color={TUI_COLORS.textMuted}>{' edit plan'}</Text>
          </Text>
          <Text>
            <Text color={TUI_COLORS.error} bold>[n]</Text>
            <Text color={TUI_COLORS.textMuted}>{' reject'}</Text>
          </Text>
        </Box>
        <Box flexDirection="row">
          <Text color={TUI_COLORS.cyan}>{'->'}</Text>
          <Text color={TUI_COLORS.textMuted}>
            {' pressing '}
            <Text color={TUI_COLORS.cyan}>[y]</Text>
            {' will switch to '}
            <Text color={TUI_COLORS.cyan}>execute mode</Text>
            {' and run all '}
            <Text color={TUI_COLORS.textPrimary}>{`${steps.length} steps`}</Text>
            {planModel !== '-' ? ' with ' : null}
            {planModel !== '-' ? <Text color={TUI_COLORS.mauve}>{planModel}</Text> : null}
          </Text>
        </Box>
      </Box>
    </Box>
  );
}
