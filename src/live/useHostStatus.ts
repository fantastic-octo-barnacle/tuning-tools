import { useEffect, useState } from "react";
import { host } from "../host";
import { STANDALONE_STATUS } from "../host/common";
import type { HostStatus } from "../host";

/** The host's sources, notice and features, kept current */
export function useHostStatus(): HostStatus {
  const [status, setStatus] = useState<HostStatus | null>(null);
  useEffect(() => host.watchStatus(setStatus), []);
  // watchStatus calls back at once, so this fallback only covers the first render
  return status ?? STANDALONE_STATUS;
}
