/** 极小的 DOM 助手：查询 + HTML 转义。 */

export function $(selector, root = document) {
  return root.querySelector(selector)
}

/** 把任意值转成可安全塞进 HTML 的文本（防注入）。 */
export function esc(value) {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}

/** 夹到 [lo, hi]。 */
export function clamp(value, lo, hi) {
  return value < lo ? lo : value > hi ? hi : value
}
