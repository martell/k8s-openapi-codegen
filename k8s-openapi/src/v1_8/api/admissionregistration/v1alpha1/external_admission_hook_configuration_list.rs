// Generated from definition io.k8s.api.admissionregistration.v1alpha1.ExternalAdmissionHookConfigurationList

/// ExternalAdmissionHookConfigurationList is a list of ExternalAdmissionHookConfiguration.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ExternalAdmissionHookConfigurationList {
    /// APIVersion defines the versioned schema of this representation of an object. Servers should convert recognized schemas to the latest internal value, and may reject unrecognized values. More info: https://git.k8s.io/community/contributors/devel/api-conventions.md#resources
    #[serde(rename = "apiVersion")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,

    /// List of ExternalAdmissionHookConfiguration.
    pub items: Vec<::v1_8::api::admissionregistration::v1alpha1::ExternalAdmissionHookConfiguration>,

    /// Kind is a string value representing the REST resource this object represents. Servers may infer this from the endpoint the client submits requests to. Cannot be updated. In CamelCase. More info: https://git.k8s.io/community/contributors/devel/api-conventions.md#types-kinds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Standard list metadata. More info: https://git.k8s.io/community/contributors/devel/api-conventions.md#types-kinds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<::v1_8::apimachinery::pkg::apis::meta::v1::ListMeta>,
}