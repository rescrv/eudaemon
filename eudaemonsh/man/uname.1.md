------------------------------------------------------------------------

*UNAME*(1) General Commands Manual *UNAME*(1)

**NAME**

uname — print eudaemonsh system identification

**SYNOPSIS**

**uname** \[**−a**\] \[**−s**\] \[**−r**\] \[**−v**\] \[**−m**\] \[**−o**\]

**DESCRIPTION**

The **uname** utility writes eudaemonsh system identification to the
standard output. This command is designed for an LLM to identify the
eudaemonsh environment it is operating in.

The options are as follows:

**−a**

Print all information, equivalent to **−srvmo**.

**−s**

Print the system name (eudaemonsh).

**−r**

Print the release version.

**−v**

Print the build version information.

**−m**

Print the machine hardware name (host architecture).

**−o**

Print the operating system name (host OS).

If no options are specified, **−s** is assumed.

**EXIT STATUS**

The **uname** utility exits 0 on success, and \>0 if an error occurs.

**EXAMPLES**

Print all system information:

\$ uname -a  
eudaemonsh 0.1.0 0.1.0 aarch64 macos

Print only the system name:

\$ uname -s  
eudaemonsh

Print the release version and host OS:

\$ uname -ro  
0.1.0 macos

**SEE ALSO**

*env*(1)

**HISTORY**

The **uname** utility is a eudaemonsh-specific implementation designed
for LLM consumption.

------------------------------------------------------------------------
