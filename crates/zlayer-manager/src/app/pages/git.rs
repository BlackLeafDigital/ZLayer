//! Git repositories page
//!
//! Displays git repository list with `DaisyUI` table styling.

use leptos::prelude::*;

/// Mock git repository data for placeholder display.
struct GitRepository {
    name: &'static str,
    url: &'static str,
    branch: &'static str,
    last_sync: &'static str,
    auto_deploy: bool,
}

/// Get mock git repositories for placeholder display.
fn mock_repositories() -> Vec<GitRepository> {
    vec![
        GitRepository {
            name: "frontend-app",
            url: "https://github.com/example/frontend-app.git",
            branch: "main",
            last_sync: "2025-01-16 14:30",
            auto_deploy: true,
        },
        GitRepository {
            name: "backend-api",
            url: "https://github.com/example/backend-api.git",
            branch: "production",
            last_sync: "2025-01-16 12:15",
            auto_deploy: true,
        },
        GitRepository {
            name: "config-repo",
            url: "https://gitlab.com/example/config-repo.git",
            branch: "master",
            last_sync: "2025-01-15 09:45",
            auto_deploy: false,
        },
    ]
}

/// Git repositories page component
///
/// Displays a table of git repositories with an "Add Repository" button.
#[component]
pub fn Git() -> impl IntoView {
    let repositories = mock_repositories();

    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-3xl font-bold">"Git Repositories"</h1>
                <button class="btn btn-primary">"Add Repository"</button>
            </div>

            <div class="overflow-x-auto">
                <table class="table table-zebra w-full">
                    <thead>
                        <tr>
                            <th>"Name"</th>
                            <th>"URL"</th>
                            <th>"Branch"</th>
                            <th>"Last Sync"</th>
                            <th>"Auto-Deploy"</th>
                            <th>"Actions"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {repositories
                            .into_iter()
                            .map(|repo| {
                                let auto_deploy_badge = if repo.auto_deploy {
                                    "badge badge-success"
                                } else {
                                    "badge badge-ghost"
                                };
                                let auto_deploy_text = if repo.auto_deploy {
                                    "Enabled"
                                } else {
                                    "Disabled"
                                };
                                view! {
                                    <tr>
                                        <td class="font-medium">{repo.name}</td>
                                        <td class="text-sm opacity-70">{repo.url}</td>
                                        <td>
                                            <span class="badge badge-outline">{repo.branch}</span>
                                        </td>
                                        <td>{repo.last_sync}</td>
                                        <td>
                                            <span class=auto_deploy_badge>{auto_deploy_text}</span>
                                        </td>
                                        <td>
                                            <button class="btn btn-ghost btn-xs">"Sync"</button>
                                            <button class="btn btn-ghost btn-xs">"Edit"</button>
                                            <button class="btn btn-ghost btn-xs text-error">"Delete"</button>
                                        </td>
                                    </tr>
                                }
                            })
                            .collect::<Vec<_>>()}
                    </tbody>
                </table>
            </div>
        </div>
    }
}
