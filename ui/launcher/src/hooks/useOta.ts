import { useCallback, useEffect, useState } from 'react';
import {
  launcherInvoke,
  onOtaStatus,
  type OtaStatus,
} from '../launcher-bridge';

export function useOta(): {
  status: OtaStatus | null;
  busy: boolean;
  check: () => Promise<OtaStatus>;
  apply: () => Promise<void>;
  dismiss: () => Promise<void>;
  openRelease: () => Promise<void>;
  setAutoCheck: (enabled: boolean) => Promise<void>;
} {
  const [status, setStatus] = useState<OtaStatus | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void launcherInvoke<OtaStatus>('ota.getStatus')
      .then(setStatus)
      .catch(() => null);
    return onOtaStatus((payload) => setStatus(payload));
  }, []);

  const check = useCallback(async () => {
    setBusy(true);
    try {
      const next = await launcherInvoke<OtaStatus>('ota.check');
      setStatus(next);
      return next;
    } finally {
      setBusy(false);
    }
  }, []);

  const apply = useCallback(async () => {
    setBusy(true);
    try {
      const next = await launcherInvoke<OtaStatus>('ota.apply');
      setStatus(next);
    } catch (err) {
      try {
        const next = await launcherInvoke<OtaStatus>('ota.getStatus');
        setStatus(next);
      } catch {
        /* ignore */
      }
      throw err;
    } finally {
      setBusy(false);
    }
  }, []);

  const dismiss = useCallback(async () => {
    const next = await launcherInvoke<OtaStatus>('ota.dismiss');
    setStatus(next);
  }, []);

  const openRelease = useCallback(async () => {
    const next = await launcherInvoke<OtaStatus>('ota.openRelease');
    setStatus(next);
  }, []);

  const setAutoCheck = useCallback(async (enabled: boolean) => {
    await launcherInvoke('settings.set', [{ otaAutoCheck: enabled }]);
    const next = await launcherInvoke<OtaStatus>('ota.getStatus');
    setStatus(next);
  }, []);

  return { status, busy, check, apply, dismiss, openRelease, setAutoCheck };
}
