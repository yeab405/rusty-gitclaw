use std::env;

/// Get API key for provider from known environment variables.
pub fn get_env_api_key(provider: &str) -> Option<String> {
    match provider {
        "github-copilot" => env::var("COPILOT_GITHUB_TOKEN")
            .or_else(|_| env::var("GH_TOKEN"))
            .or_else(|_| env::var("GITHUB_TOKEN"))
            .ok(),

        "anthropic" => env::var("ANTHROPIC_OAUTH_TOKEN")
            .or_else(|_| env::var("ANTHROPIC_API_KEY"))
            .ok(),

        "google-vertex" => {
            let has_project =
                env::var("GOOGLE_CLOUD_PROJECT").is_ok() || env::var("GCLOUD_PROJECT").is_ok();
            let has_location = env::var("GOOGLE_CLOUD_LOCATION").is_ok();
            // Check for ADC credentials
            let has_creds = env::var("GOOGLE_APPLICATION_CREDENTIALS")
                .ok()
                .map(|p| std::path::Path::new(&p).exists())
                .unwrap_or_else(|| {
                    if let Some(home) = dirs_home() {
                        std::path::Path::new(&home)
                            .join(".config/gcloud/application_default_credentials.json")
                            .exists()
                    } else {
                        false
                    }
                });

            if has_creds && has_project && has_location {
                Some("<authenticated>".to_string())
            } else {
                None
            }
        }

        "amazon-bedrock" => {
            if env::var("AWS_PROFILE").is_ok()
                || (env::var("AWS_ACCESS_KEY_ID").is_ok()
                    && env::var("AWS_SECRET_ACCESS_KEY").is_ok())
                || env::var("AWS_BEARER_TOKEN_BEDROCK").is_ok()
                || env::var("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI").is_ok()
                || env::var("AWS_CONTAINER_CREDENTIALS_FULL_URI").is_ok()
                || env::var("AWS_WEB_IDENTITY_TOKEN_FILE").is_ok()
            {
                Some("<authenticated>".to_string())
            } else {
                None
            }
        }

        _ => {
            let env_var = match provider {
                "openai" => "OPENAI_API_KEY",
                "azure-openai-responses" => "AZURE_OPENAI_API_KEY",
                "google" => "GEMINI_API_KEY",
                "groq" => "GROQ_API_KEY",
                "cerebras" => "CEREBRAS_API_KEY",
                "xai" => "XAI_API_KEY",
                "openrouter" => "OPENROUTER_API_KEY",
                "vercel-ai-gateway" => "AI_GATEWAY_API_KEY",
                "zai" => "ZAI_API_KEY",
                "mistral" => "MISTRAL_API_KEY",
                "minimax" => "MINIMAX_API_KEY",
                "minimax-cn" => "MINIMAX_CN_API_KEY",
                "huggingface" => "HF_TOKEN",
                "opencode" | "opencode-go" => "OPENCODE_API_KEY",
                "kimi-coding" => "KIMI_API_KEY",
                _ => return None,
            };
            env::var(env_var).ok()
        }
    }
}

fn dirs_home() -> Option<String> {
    env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .ok()
}
