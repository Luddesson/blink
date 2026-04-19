param(
    [ValidateSet("validate", "up", "down", "ps")]
    [string]$Action = "validate"
)

$ErrorActionPreference = "Stop"
$composeFile = "infra\oss-eval-stack\docker-compose.oss-eval.yml"
$envFile = "infra\oss-eval-stack\.env.eval"

if (-not (Test-Path $envFile)) {
    Copy-Item "infra\oss-eval-stack\.env.eval.example" $envFile
}

switch ($Action) {
    "validate" {
        docker compose --env-file $envFile -f $composeFile config | Out-Null
        Write-Host "Compose config is valid."
    }
    "up" {
        docker compose --env-file $envFile -f $composeFile up -d
    }
    "down" {
        docker compose --env-file $envFile -f $composeFile down
    }
    "ps" {
        docker compose --env-file $envFile -f $composeFile ps
    }
}
