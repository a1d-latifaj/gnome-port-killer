# UDP filter runtime test

Purpose:
- Verify successful UDP bind generates one invalidation.
- Verify closing the tracked UDP socket generates one invalidation.
- Verify unrelated UDP socket closures do not generate invalidations.
- Confirm TCP events continue working.

Constraints:
- Do not stop or restart port-killer-events.service.
- Do not replace /run/port-killer/events.sock.
- Do not modify GNOME extension.
- Run against the locally compiled eBPF object.
