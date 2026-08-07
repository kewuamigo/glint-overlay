import { useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { applyQueuedPluginStyles } from '../plugin-styles';

interface PluginSurfaceProps {
  pluginId: string;
  className?: string;
  privileged?: boolean;
  children: ReactNode;
}

export function PluginSurface({
  pluginId,
  className,
  privileged = false,
  children,
}: PluginSurfaceProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [mount, setMount] = useState<HTMLElement | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    if (privileged) {
      host.dataset.appRoot = pluginId;
      applyQueuedPluginStyles(pluginId, host);
      if (className) host.className = className;
      host.style.cssText =
        'display:flex;flex-direction:column;width:100%;height:100%;min-height:0;';
      setMount(host);
      return;
    }

    const shadow = host.shadowRoot ?? host.attachShadow({ mode: 'open' });
    let root = shadow.querySelector<HTMLElement>('[data-plugin-mount]');
    if (!root) {
      root = document.createElement('div');
      root.dataset.pluginMount = '';
      if (className) root.className = className;
      shadow.appendChild(root);
    } else if (className) {
      root.className = className;
    }
    // Height chain so plugin roots (e.g. .ach-trophy-grid overflow:auto) can scroll.
    root.style.cssText =
      'display:flex;flex-direction:column;width:100%;height:100%;min-height:0;box-sizing:border-box;';

    applyQueuedPluginStyles(pluginId, shadow);
    setMount(root);
  }, [pluginId, className, privileged]);

  return (
    <>
      {/*
        The host div MUST be keyed per app + mode: React otherwise reuses the
        same DOM node when switching tabs. attachShadow() is irreversible, so
        a div that hosted a shadow-DOM app (e.g. metrics) can never render
        light-DOM children again — a privileged app (browser) portaled into it
        would attach but stay completely invisible.
      */}
      <div
        key={`${pluginId}:${privileged ? 'privileged' : 'shadow'}`}
        ref={hostRef}
        className={privileged ? 'app-privileged-host' : 'plugin-shadow-host'}
      />
      {mount ? createPortal(children, mount) : null}
    </>
  );
}
