use serde::Deserialize;

use crate::models::AssetId;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDeveloperProductIconResponse {
    pub image_asset_id: AssetId,
}

// product_id is the purchasable id used with MarketplaceService prompts
// (mantle's asset_id output); developer_product_id is the legacy internal id.
pub struct CreateDeveloperProductResponse {
    pub product_id: AssetId,
    pub developer_product_id: AssetId,
}

#[derive(Clone)]
pub struct ListDeveloperProductResponseItem {
    pub product_id: AssetId,
    pub developer_product_id: AssetId,
    pub name: String,
    pub description: Option<String>,
    pub icon_image_asset_id: Option<AssetId>,
    pub price_in_robux: u32,
}
