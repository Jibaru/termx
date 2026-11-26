# Restores the default handling of Ctrl+C for this process and everything it
# starts, then runs the command given as arguments. See run.mjs.
$signature = '[DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetConsoleCtrlHandler(IntPtr handler, bool add);'
$kernel32 = Add-Type -Namespace TermxE2e -Name Kernel32 -MemberDefinition $signature -PassThru
if (-not $kernel32::SetConsoleCtrlHandler([IntPtr]::Zero, $false)) {
    Write-Error "the default handling of Ctrl+C could not be restored"
    exit 1
}

$command, $rest = $args
& $command @rest
exit $LASTEXITCODE
