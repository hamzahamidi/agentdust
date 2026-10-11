# WSL2 runtime limits

AgentDust runs as a Linux process inside one Ubuntu 24.04 WSL2 distribution. The agent and AgentDust must run in that same distribution so process discovery, identity checks and signals use one Linux process namespace.

Keep the AgentDust data directory on the distribution's writable ext4 filesystem, such as a directory under `/home`. AgentDust refuses setup when the data directory is on `/mnt/c` or another unsupported filesystem.

Windows host processes are outside the Linux process inventory. Start Linux agents from WSL when their helper processes need to be diagnosed or cleaned up. AgentDust cannot identify or stop Windows host processes.

The automatic worker uses the systemd user service while the distribution is running. Stopping WSL also stops that worker. Enabling systemd does not keep the distribution alive.

WSL2 uses the Linux x86_64 package. It is not native Windows support. Native Windows process discovery, listener attribution, installation and safe per-process termination remain a separate feasibility milestone.
