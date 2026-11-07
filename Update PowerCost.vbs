' Double-click to update PowerCost. Only the progress window appears: this launcher starts
' PowerShell with no console window at all (a plain "hidden" PowerShell still shows a Windows
' Terminal window on Windows 11). Extra arguments are passed on, e.g. -DryRun or -NoInstall.
Option Explicit
Dim sh, fso, dir, args, i
Set sh = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
dir = fso.GetParentFolderName(WScript.ScriptFullName)
args = ""
For i = 0 To WScript.Arguments.Count - 1
    args = args & " " & WScript.Arguments(i)
Next
sh.Run "powershell.exe -NoProfile -ExecutionPolicy Bypass -STA -WindowStyle Hidden -File """ & dir & "\scripts\update.ps1""" & args, 0, False
