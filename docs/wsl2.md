# WSL2 runtime limits

WSL2 support in AgentDust 1.7.1 requires the exact Linux x86_64 archive and npm package to pass WSL2 checks in the release workflow before publication. The target is Ubuntu 24.04 x86_64. The agent and AgentDust must run in the same distribution, and the data directory must be on writable ext4.

AgentDust runs as a Linux process inside one Ubuntu 24.04 WSL2 distribution. The agent and AgentDust must run in that same distribution so process discovery, identity checks and signals use one Linux process namespace.

Keep the AgentDust data directory on the distribution's writable ext4 filesystem, such as a directory under `/home`. AgentDust refuses setup when the data directory is on `/mnt/c` or another unsupported filesystem.

Windows host processes are outside the Linux process inventory. Start Linux agents from WSL when their helper processes need to be diagnosed or cleaned up. AgentDust cannot identify or stop Windows host processes.

The automatic worker uses the systemd user service while the distribution is running. Stopping WSL also stops that worker. Enabling systemd does not keep the distribution alive.

WSL2 uses the Linux x86_64 package. It is not native Windows support. Native Windows process discovery, listener attribution, installation and safe per-process termination remain a separate feasibility milestone.
