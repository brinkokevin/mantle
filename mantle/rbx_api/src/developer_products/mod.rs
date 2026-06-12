pub mod models;

use std::path::PathBuf;

use reqwest::multipart::Form;
use serde_json::Value;

use crate::{
    errors::{RobloxApiError, RobloxApiResult},
    helpers::{get_file_part, handle, handle_as_json},
    models::AssetId,
    RobloxApi,
};

use self::models::{
    CreateDeveloperProductIconResponse, CreateDeveloperProductResponse,
    ListDeveloperProductResponseItem,
};

// Roblox retired the legacy developer-products v1 create/update/list endpoints
// (410 Gone as of 2026-04-23). These functions use the v2 API instead:
// https://create.roblox.com/docs/cloud/reference/features/developer-products.md
// The v2 endpoints accept cookie authentication in addition to API keys.

fn value_to_asset_id(value: &Value) -> Option<AssetId> {
    match value {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn parse_list_item(item: &Value) -> Option<ListDeveloperProductResponseItem> {
    let product_id = item.get("productId").and_then(value_to_asset_id)?;
    Some(ListDeveloperProductResponseItem {
        product_id,
        // The v2 listing no longer exposes the legacy DeveloperProductId; nothing
        // consumes it functionally, so mirror the purchasable id.
        developer_product_id: item
            .get("developerProductId")
            .and_then(value_to_asset_id)
            .unwrap_or(product_id),
        name: item.get("name")?.as_str()?.to_owned(),
        description: item
            .get("description")
            .and_then(|d| d.as_str())
            .map(|d| d.to_owned()),
        icon_image_asset_id: item.get("iconImageAssetId").and_then(value_to_asset_id),
        price_in_robux: item
            .get("priceInformation")
            .and_then(|p| p.get("defaultPriceInRobux"))
            .and_then(|p| p.as_u64())
            .or_else(|| item.get("priceInRobux").and_then(|p| p.as_u64()))
            .unwrap_or(0) as u32,
    })
}

impl RobloxApi {
    pub async fn create_developer_product_icon(
        &self,
        developer_product_id: AssetId,
        icon_file: PathBuf,
    ) -> RobloxApiResult<CreateDeveloperProductIconResponse> {
        let res = self
            .csrf_token_store
            .send_request(|| async {
                Ok(self
                    .client
                    .post(format!(
                        "https://apis.roblox.com/developer-products/v1/developer-products/{}/image",
                        developer_product_id
                    ))
                    .multipart(Form::new().part("imageFile", get_file_part(&icon_file).await?)))
            })
            .await;

        handle_as_json(res).await
    }

    pub async fn create_developer_product(
        &self,
        experience_id: AssetId,
        name: String,
        price: u32,
        description: String,
    ) -> RobloxApiResult<CreateDeveloperProductResponse> {
        let res = self
            .csrf_token_store
            .send_request(|| async {
                Ok(self
                    .client
                    .post(format!(
                        "https://apis.roblox.com/developer-products/v2/universes/{}/developer-products",
                        experience_id
                    ))
                    .multipart(
                        Form::new()
                            .text("name", name.clone())
                            .text("description", description.clone())
                            .text("price", price.to_string())
                            .text("isForSale", "true"),
                    ))
            })
            .await;

        let body = match handle(res).await {
            Ok(response) => response.json::<Value>().await.unwrap_or(Value::Null),
            // A product with this name may already exist on Roblox without being
            // tracked in the mantle state (e.g. created before a state rebuild, or
            // by a partially-failed deploy). Adopt it by name instead of failing.
            Err(RobloxApiError::Roblox { reason, .. })
                if reason.contains("DuplicateProductName") =>
            {
                Value::Null
            }
            Err(error) => return Err(error),
        };

        // The v2 create response schema is in beta; take the ids from the response
        // when present, otherwise resolve them from the listing endpoint.
        let product_id = body.get("productId").and_then(value_to_asset_id);
        let developer_product_id = body.get("developerProductId").and_then(value_to_asset_id);
        if let (Some(product_id), Some(developer_product_id)) = (product_id, developer_product_id)
        {
            return Ok(CreateDeveloperProductResponse {
                product_id,
                developer_product_id,
            });
        }

        let product = self
            .find_developer_product_by_name(experience_id, &name)
            .await?;
        Ok(CreateDeveloperProductResponse {
            product_id: product.product_id,
            developer_product_id: product.developer_product_id,
        })
    }

    async fn list_developer_products_page(
        &self,
        experience_id: AssetId,
        page_token: Option<String>,
    ) -> RobloxApiResult<Value> {
        let res = self
            .csrf_token_store
            .send_request(|| async {
                let mut req = self
                    .client
                    .get(format!(
                        "https://apis.roblox.com/developer-products/v2/universes/{}/developer-products/creator",
                        experience_id
                    ))
                    .query(&[("pageSize", "50")]);
                if let Some(token) = &page_token {
                    req = req.query(&[("pageToken", token)]);
                }
                Ok(req)
            })
            .await;

        handle_as_json(res).await
    }

    pub async fn get_all_developer_products(
        &self,
        experience_id: AssetId,
    ) -> RobloxApiResult<Vec<ListDeveloperProductResponseItem>> {
        let mut all_products = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let page = self
                .list_developer_products_page(experience_id, page_token.clone())
                .await?;

            let items = page
                .get("developerProducts")
                .or_else(|| page.get("developerProductsOverview"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            all_products.extend(items.iter().filter_map(parse_list_item));

            page_token = page
                .get("nextPageToken")
                .or_else(|| page.get("nextPageCursor"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_owned());

            if page_token.is_none() || items.is_empty() {
                break;
            }
        }

        Ok(all_products)
    }

    pub async fn find_developer_product_by_name(
        &self,
        experience_id: AssetId,
        name: &str,
    ) -> RobloxApiResult<ListDeveloperProductResponseItem> {
        let products = self.get_all_developer_products(experience_id).await?;
        products
            .into_iter()
            .find(|product| product.name == name)
            .ok_or_else(|| RobloxApiError::Roblox {
                status_code: reqwest::StatusCode::NOT_FOUND,
                reason: format!("Developer product {} not found in universe listing", name),
            })
    }

    pub async fn update_developer_product(
        &self,
        experience_id: AssetId,
        product_id: AssetId,
        name: String,
        price: u32,
        description: String,
    ) -> RobloxApiResult<()> {
        let res = self
            .csrf_token_store
            .send_request(|| async {
                Ok(self
                    .client
                    .patch(format!(
                        "https://apis.roblox.com/developer-products/v2/universes/{}/developer-products/{}",
                        experience_id, product_id
                    ))
                    .multipart({
                        let form = Form::new()
                            .text("name", name.clone())
                            .text("description", description.clone());
                        // v2 rejects price <= 0; price 0 (mantle's delete/deprecate
                        // convention) translates to taking the product off sale.
                        if price > 0 {
                            form.text("price", price.to_string()).text("isForSale", "true")
                        } else {
                            form.text("isForSale", "false")
                        }
                    }))
            })
            .await;

        handle(res).await?;

        Ok(())
    }
}
