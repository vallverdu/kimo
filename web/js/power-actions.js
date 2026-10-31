// A machine's smart plug, as a person asks for it: cutting power gets a plain warning first,
// and when the coordinator says something runs there, a second one naming what would be lost.

const ASK = {
  off: name => `Cut the power to ${name}? Anything running there stops at once, like pulling the cable.`,
  cycle: name => `Restart ${name} by cutting its power? Anything running there stops at once, like pulling the cable.`,
};
const DONE = { on: 'on', off: 'off', cycle: 'restarted' };

/** The plug's action, after asking. Resolves to what the plug says, or null if you said no. */
export async function plugAction(fleet, id, action) {
  if (ASK[action] && !confirm(ASK[action](id))) return null;
  try {
    return await fleet.plugAction(id, action);
  } catch (e) {
    if (e.status !== 409 || !confirm(`${e.message}\n\nCut the power anyway?`)) throw e;
    return fleet.plugAction(id, action, { force: true });
  }
}

export const plugDone = (id, action) => `Plug of ${id}: ${DONE[action] || action}`;

/** One line about a plug: `on, 41 W, 120 Wh today`. */
export function plugLine(plug) {
  if (!plug) return '';
  if (!plug.status) return plug.error || 'not read yet';
  const s = plug.status;
  return [s.on ? 'on' : 'off', s.watts != null ? `${s.watts} W` : '', s.today_wh != null ? `${s.today_wh} Wh today` : ''].filter(Boolean).join(', ');
}

/** The plug's buttons, for `data-plug` clicks. A plug reached through its own machine can only restart. */
export function plugButtons(id, plug) {
  if (!plug) return '';
  const selfReached = plug.via === id;
  const on = plug.status?.on;
  return [
    on === false ? '<button class="ui-button k-small" type="button" data-plug="on">plug on</button>' : '',
    on !== false ? '<button class="ui-button ghost k-small k-danger" type="button" data-plug="cycle">restart by plug</button>' : '',
    on !== false && !selfReached ? '<button class="ui-button ghost k-small k-danger" type="button" data-plug="off">plug off</button>' : '',
  ].join('');
}
