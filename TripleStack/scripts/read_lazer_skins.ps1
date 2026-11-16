# Adapted from the user-provided LazerUnSlopper skin reader.
param(
    [Parameter(Mandatory=$true)][string]$Database,
    [Parameter(Mandatory=$true)][string]$RealmAssembly,
    [Parameter(Mandatory=$true)][string]$OutputFile
)
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'PowerShell 7 is required.' }
[void][System.Reflection.Assembly]::LoadFrom($RealmAssembly)
$config = [Realms.RealmConfiguration]::new($Database)
$config.IsDynamic = $true
$config.IsReadOnly = $true
$realm = [Realms.Realm]::GetInstance($config)
try {
    $skins = [System.Collections.Generic.List[object]]::new()
    foreach ($skin in $realm.DynamicApi.All('Skin')) {
        if ($skin.DynamicApi.Get[bool]('DeletePending') -or $skin.DynamicApi.Get[bool]('Protected')) { continue }
        $files = [System.Collections.Generic.List[object]]::new()
        foreach ($usage in $skin.DynamicApi.GetList[Realms.IRealmObjectBase]('Files')) {
            $file = $usage.DynamicApi.Get[Realms.IRealmObjectBase]('File')
            $files.Add(@{name=$usage.DynamicApi.Get[string]('Filename'); hash=$file.DynamicApi.Get[string]('Hash')})
        }
        $skins.Add(@{
            key = $skin.DynamicApi.Get[guid]('ID').ToString()
            name = $skin.DynamicApi.Get[string]('Name')
            creator = $skin.DynamicApi.Get[string]('Creator')
            instantiation = $skin.DynamicApi.Get[string]('InstantiationInfo')
            files = $files.ToArray()
        })
    }
    @{version=1; skins=$skins.ToArray()} | ConvertTo-Json -Depth 8 -Compress | Set-Content -LiteralPath $OutputFile -Encoding utf8
} finally {
    $realm.Dispose()
}
