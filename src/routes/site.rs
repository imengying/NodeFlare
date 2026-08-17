use worker::{Method, Request, Response, Result};

use crate::auth::verify_theme_preview_proof;
use crate::routes::{RouteContext, RouteOutcome};
use crate::{
    db, remote_theme_preview_response, remote_theme_response, secure_public_response, theme,
    ADMIN_HTML, ADMIN_SCRIPT, ADMIN_STYLE,
};

/// 管理端内嵌资源（编译进 wasm，不受主题影响）。
pub(crate) fn embedded_asset(path: &str) -> Result<Option<Response>> {
    match path {
        "/admin" | "/admin/" | "/admin/index.html" => {
            crate::embedded_admin_response(ADMIN_HTML, "text/html; charset=utf-8").map(Some)
        }
        "/admin-assets/admin.js" => {
            crate::embedded_admin_response(ADMIN_SCRIPT, "application/javascript; charset=utf-8")
                .map(Some)
        }
        "/admin-assets/admin.css" => {
            crate::embedded_admin_response(ADMIN_STYLE, "text/css; charset=utf-8").map(Some)
        }
        _ => Ok(None),
    }
}

/// 主题预览与远程主题内容；未匹配时归还请求，由调用方落到静态资源。
pub(crate) async fn route(req: Request, ctx: &RouteContext) -> Result<RouteOutcome> {
    if req.method() != Method::Get {
        return Ok(RouteOutcome::Unmatched(req));
    }
    let path = req.path();

    if let Some(preview_path) = path.strip_prefix("/__theme-preview/") {
        let (proof, relative) = preview_path.split_once('/').unwrap_or((preview_path, ""));
        let Some(theme_id) =
            verify_theme_preview_proof(proof, &ctx.env, &ctx.settings.admin_password_hash)
        else {
            return Ok(RouteOutcome::Handled(secure_public_response(
                Response::error("主题预览链接已过期", 403)?,
            )?));
        };
        let Some(url) = db::theme_url(&ctx.database, &theme_id).await? else {
            return Ok(RouteOutcome::Handled(secure_public_response(
                Response::error("主题不存在", 404)?,
            )?));
        };
        let prefix = format!("/__theme-preview/{proof}");
        return Ok(RouteOutcome::Handled(
            remote_theme_preview_response(relative, &url, &prefix).await?,
        ));
    }

    if let Some(relative) = path.strip_prefix("/__theme-active/") {
        if ctx.settings.active_theme_id == theme::BUILTIN_THEME_ID {
            return Ok(RouteOutcome::Handled(Response::error(
                "远程主题未启用",
                404,
            )?));
        }
        let Some(url) = db::theme_url(&ctx.database, &ctx.settings.active_theme_id).await? else {
            return Ok(RouteOutcome::Handled(Response::error("主题不存在", 404)?));
        };
        let remote_path = format!("/{relative}");
        return Ok(RouteOutcome::Handled(
            remote_theme_response(&remote_path, &url)
                .await?
                .unwrap_or(Response::error("主题资源不存在", 404)?),
        ));
    }

    if ctx.settings.active_theme_id != theme::BUILTIN_THEME_ID {
        if let Some(url) = db::theme_url(&ctx.database, &ctx.settings.active_theme_id).await? {
            if let Some(response) = remote_theme_response(&path, &url).await? {
                return Ok(RouteOutcome::Handled(response));
            }
        }
    }

    Ok(RouteOutcome::Unmatched(req))
}
