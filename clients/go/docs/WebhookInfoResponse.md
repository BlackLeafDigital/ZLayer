# WebhookInfoResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Secret** | **string** | The HMAC secret to paste into the git host. | 
**Url** | **string** | Full URL to configure in the git host&#39;s webhook settings. | 

## Methods

### NewWebhookInfoResponse

`func NewWebhookInfoResponse(secret string, url string, ) *WebhookInfoResponse`

NewWebhookInfoResponse instantiates a new WebhookInfoResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewWebhookInfoResponseWithDefaults

`func NewWebhookInfoResponseWithDefaults() *WebhookInfoResponse`

NewWebhookInfoResponseWithDefaults instantiates a new WebhookInfoResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetSecret

`func (o *WebhookInfoResponse) GetSecret() string`

GetSecret returns the Secret field if non-nil, zero value otherwise.

### GetSecretOk

`func (o *WebhookInfoResponse) GetSecretOk() (*string, bool)`

GetSecretOk returns a tuple with the Secret field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSecret

`func (o *WebhookInfoResponse) SetSecret(v string)`

SetSecret sets Secret field to given value.


### GetUrl

`func (o *WebhookInfoResponse) GetUrl() string`

GetUrl returns the Url field if non-nil, zero value otherwise.

### GetUrlOk

`func (o *WebhookInfoResponse) GetUrlOk() (*string, bool)`

GetUrlOk returns a tuple with the Url field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUrl

`func (o *WebhookInfoResponse) SetUrl(v string)`

SetUrl sets Url field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


