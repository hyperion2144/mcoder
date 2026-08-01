// 设计文档 §8.6.1: 桌面端代码图谱可视化
// 通过 graph_query 工具获取节点和边，用 SVG 渲染力导向图

import React, { useState, useEffect, useRef } from 'react';
import type { WsClient } from '@mcoder/shared/rpc/client.js';
import { t } from '../i18n.js';

interface GraphNode {
  id: string;
  name: string;
  kind: string;
  x?: number;
  y?: number;
}

interface GraphEdge {
  source: string;
  target: string;
  kind: string;
}

export function GraphView({ client }: { client: WsClient }) {
  const [nodes, setNodes] = useState<GraphNode[]>([]);
  const [edges, setEdges] = useState<GraphEdge[]>([]);
  const [loading, setLoading] = useState(false);
  const svgRef = useRef<SVGSVGElement>(null);

  const loadGraph = async () => {
    setLoading(true);
    try {
      // P1-6: graph_query 必填参数是 name（substring match）
      // 传空字符串匹配所有符号，limit 限制为 50 个节点
      const result = await client.request('tool.call', {
        name: 'graph_query',
        args: { name: '', limit: 50 },
      });

      // result 可能是数组或包含 symbols 字段的对象
      const symbols: any[] = Array.isArray(result)
        ? result
        : (result.symbols || result.result || []);

      // 将每个符号作为一个节点
      const graphNodes: GraphNode[] = symbols.slice(0, 50).map((s: any, i: number) => ({
        id: s.id || (s.file ? `${s.file}:${s.line}` : `node-${i}`),
        name: s.name || `node-${i}`,
        kind: s.kind || 'unknown',
        x: 100 + (i % 8) * 120,
        y: 80 + Math.floor(i / 8) * 100,
      }));
      setNodes(graphNodes);
      setEdges([]);
    } catch (e) {
      // ignore
    }
    setLoading(false);
  };

  useEffect(() => {
    loadGraph();
  }, []);

  const colors: Record<string, string> = {
    file: '#61dafb',
    function: '#21ba45',
    class: '#f2711c',
    variable: '#a333c8',
  };

  return (
    <div className="graph-view">
      <div className="graph-view-header">
        <span>{t('ui.code_graph')}</span>
        <button onClick={loadGraph} disabled={loading}>
          {loading ? t('ui.loading') : t('ui.refresh')}
        </button>
      </div>
      <svg ref={svgRef} className="graph-svg" width="100%" height="600">
        {/* 边 */}
        {edges.map((edge, i) => {
          const source = nodes.find(n => n.id === edge.source);
          const target = nodes.find(n => n.id === edge.target);
          if (!source?.x || !source?.y || !target?.x || !target?.y) return null;
          return (
            <line
              key={`edge-${i}`}
              x1={source.x}
              y1={source.y}
              x2={target.x}
              y2={target.y}
              stroke="#444"
              strokeWidth="1"
            />
          );
        })}
        {/* 节点 */}
        {nodes.map(node => (
          <g key={node.id} transform={`translate(${node.x || 0}, ${node.y || 0})`}>
            <circle
              r="6"
              fill={colors[node.kind] || '#888'}
              stroke="#fff"
              strokeWidth="1"
            />
            <text
              x="10"
              y="4"
              fill="#ccc"
              fontSize="10"
              fontFamily="monospace"
            >
              {node.name.length > 20 ? node.name.slice(0, 20) + '...' : node.name}
            </text>
          </g>
        ))}
      </svg>
      {nodes.length === 0 && !loading && (
        <div className="graph-empty">{t('ui.no_graph_data')}</div>
      )}
    </div>
  );
}
