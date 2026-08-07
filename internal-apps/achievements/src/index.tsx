import type { PluginComponent } from '@glint/plugin-sdk';
import pluginCss from './styles.css?inline';
import { AchievementsApp } from './AchievementsApp';

const APP_ID = 'achievements';

if (typeof window !== 'undefined') {
  if (window.__goQueuePluginCss) {
    window.__goQueuePluginCss(APP_ID, pluginCss);
  } else if (typeof document !== 'undefined') {
    const style = document.createElement('style');
    style.textContent = pluginCss;
    document.head.appendChild(style);
  }
}

export const Plugin: PluginComponent = ({ api, mode }) => (
  <AchievementsApp api={api} mode={mode} />
);

export default Plugin;
