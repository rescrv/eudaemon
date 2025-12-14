------------------------------------------------------------------------

*TRUNCATE*(1) General Commands Manual *TRUNCATE*(1)

**NAME**

truncate — resize files or manage file space

**SYNOPSIS**

**truncate** \[**−c**\] **−s **

\[**+**\|**-**\|**%**\|**/**\]*size*\[**SUFFIX**\] *file ...*

**truncate** \[**−c**\] **−r ***rfile file ...* **  
truncate** \[**−c**\] **−d** \[

**−o ** *  
offset*\[**SUFFIX**\] \] **−l** *  
length*\[**SUFFIX**\] *file ...*

**DESCRIPTION**

The **truncate** utility adjusts the length of each regular file given
on the command-line, or performs space management with the given offset
and the length over a regular file given on the command-line.

The following options are available:

**−c**

Do not create files if they do not exist. The **truncate** utility does
not treat this as an error. No error messages are displayed and the exit
value is not affected.

**−r** *rfile*

Truncate or extend files to the length of the file *rfile*.

**−s**  
\[**+**\|**-**\|**%**\|**/**\]*size*\[**SUFFIX**\]

If the *size* argument is preceded by a plus sign (**+**), files will be
extended by this number of bytes. If the *size* argument is preceded by
a dash (**-**), file lengths will be reduced by no more than this number
of bytes, to a minimum length of zero bytes. If the *size* argument is
preceded by a percent sign (**%**), files will be round up to a multiple
of this number of bytes. If the *size* argument is preceded by a slash
sign (**/**), files will be round down to a multiple of this number of
bytes, to a minimum length of zero bytes. Otherwise, the *size* argument
specifies an absolute length to which all files should be extended or
reduced as appropriate.

**−d**

Zero a region in the specified file. If the underlying file system of
the given file supports hole-punching, file system space deallocation
may be performed in the operation region.

**−o** *offset*

The space management operation is performed at the given *offset* bytes
in the file. If this option is not specified, the operation is performed
at the beginning of the file.

**−l** *length*

The length of the operation range in bytes. This option must always be
specified if option **−d** is specified, and must be greater than 0.

The *size*, *offset* and *length* arguments may be suffixed with one of
**K**, **M**, **G** or **T** (either upper or lower case) to indicate a
multiple of Kilobytes, Megabytes, Gigabytes or Terabytes respectively.

Exactly one of the **−r**, **−s** and **−d** options must be specified.

If a file is made smaller, its extra data is lost. If a file is made
larger, it will be extended as if by writing bytes with the value zero.
If the file does not exist, it is created unless the **−c** option is
specified.

Note that, while truncating a file causes space on disk to be freed,
extending a file does not cause space to be allocated. To extend a file
and actually allocate the space, it is necessary to explicitly write
data to it, using (for example) the shell’s ‘\>\>’ redirection syntax,
or *dd*(1).

**EXIT STATUS**

The **truncate** utility exits 0 on success, and \>0 if an error occurs.
If the operation fails for an argument, **truncate** will issue a
diagnostic and continue processing the remaining arguments.

**EXAMPLES**

Adjust the size of the file *test_file* to 10 megabytes but do not
create it if it does not exist:

truncate -c -s 10M test_file

Same as above but create the file if it does not exist:

truncate -s +10M test_file  
ls -lh test_file  
-rw-r--r-- 1 root wheel 10M Jul 22 18:48 test_file

Adjust the size of *test_file* to the size of the kernel and create
another file *test_file2* with the same size:

truncate -r /boot/kernel/kernel test_file test_file2  
ls -lh /boot/kernel/kernel test_file\*  
-r--r--r-- 1 root wheel 30M May 15 14:18 /boot/kernel/kernel  
-rw-r--r-- 1 root wheel 30M Jul 22 19:15 test_file  
-rw-r--r-- 1 root wheel 30M Jul 22 19:15 test_file2

Increase the size of the file *test_file* by 5 megabytes but do not
create it if it does not exist:

truncate -s +5M test_file  
ls -l test_file\*  
-rw-r--r-- 1 root wheel 36595432 Sep 20 19:17 test_file  
-rw-r--r-- 1 root wheel 31352552 Sep 20 19:15 test_file2

Reduce the size of the file *test_file* by 5 megabytes:

truncate -s -5M test_file  
ls -lh test_file\*  
-rw-r--r-- 1 root wheel 25M Jul 22 19:17 test_file  
-rw-r--r-- 1 root wheel 30M Jul 22 19:15 test_file2

**SEE ALSO**

*dd*(1), *touch*(1), *fspacectl*(2), *truncate*(2)

**STANDARDS**

The **truncate** utility conforms to no known standards.

**HISTORY**

The **truncate** utility first appeared in FreeBSD 4.2.

**AUTHORS**

The **truncate** utility was written by Sheldon Hearn
\<*sheldonh@starjuice.net*\>. Hole-punching support of this utility was
developed by Ka Ho Ng \<*khng@FreeBSD.org*\>. GNU July 9, 2025
*TRUNCATE*(1)

------------------------------------------------------------------------
