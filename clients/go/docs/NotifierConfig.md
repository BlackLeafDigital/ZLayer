# NotifierConfig

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Type** | **string** |  | 
**WebhookUrl** | **string** | Discord webhook URL. | 
**Headers** | Pointer to **map[string]string** | Extra headers to send with the request. | [optional] 
**Method** | Pointer to **string** | HTTP method (defaults to &#x60;\&quot;POST\&quot;&#x60;). | [optional] 
**Url** | **string** | Target URL. | 
**From** | **string** | Sender email address. | 
**Host** | **string** | SMTP server host. | 
**Password** | **string** | SMTP password. | 
**Port** | **int32** | SMTP server port. | 
**To** | **[]string** | Recipient email addresses. | 
**Username** | **string** | SMTP username. | 

## Methods

### NewNotifierConfig

`func NewNotifierConfig(type_ string, webhookUrl string, url string, from string, host string, password string, port int32, to []string, username string, ) *NotifierConfig`

NewNotifierConfig instantiates a new NotifierConfig object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNotifierConfigWithDefaults

`func NewNotifierConfigWithDefaults() *NotifierConfig`

NewNotifierConfigWithDefaults instantiates a new NotifierConfig object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetType

`func (o *NotifierConfig) GetType() string`

GetType returns the Type field if non-nil, zero value otherwise.

### GetTypeOk

`func (o *NotifierConfig) GetTypeOk() (*string, bool)`

GetTypeOk returns a tuple with the Type field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetType

`func (o *NotifierConfig) SetType(v string)`

SetType sets Type field to given value.


### GetWebhookUrl

`func (o *NotifierConfig) GetWebhookUrl() string`

GetWebhookUrl returns the WebhookUrl field if non-nil, zero value otherwise.

### GetWebhookUrlOk

`func (o *NotifierConfig) GetWebhookUrlOk() (*string, bool)`

GetWebhookUrlOk returns a tuple with the WebhookUrl field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetWebhookUrl

`func (o *NotifierConfig) SetWebhookUrl(v string)`

SetWebhookUrl sets WebhookUrl field to given value.


### GetHeaders

`func (o *NotifierConfig) GetHeaders() map[string]string`

GetHeaders returns the Headers field if non-nil, zero value otherwise.

### GetHeadersOk

`func (o *NotifierConfig) GetHeadersOk() (*map[string]string, bool)`

GetHeadersOk returns a tuple with the Headers field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHeaders

`func (o *NotifierConfig) SetHeaders(v map[string]string)`

SetHeaders sets Headers field to given value.

### HasHeaders

`func (o *NotifierConfig) HasHeaders() bool`

HasHeaders returns a boolean if a field has been set.

### GetMethod

`func (o *NotifierConfig) GetMethod() string`

GetMethod returns the Method field if non-nil, zero value otherwise.

### GetMethodOk

`func (o *NotifierConfig) GetMethodOk() (*string, bool)`

GetMethodOk returns a tuple with the Method field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMethod

`func (o *NotifierConfig) SetMethod(v string)`

SetMethod sets Method field to given value.

### HasMethod

`func (o *NotifierConfig) HasMethod() bool`

HasMethod returns a boolean if a field has been set.

### GetUrl

`func (o *NotifierConfig) GetUrl() string`

GetUrl returns the Url field if non-nil, zero value otherwise.

### GetUrlOk

`func (o *NotifierConfig) GetUrlOk() (*string, bool)`

GetUrlOk returns a tuple with the Url field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUrl

`func (o *NotifierConfig) SetUrl(v string)`

SetUrl sets Url field to given value.


### GetFrom

`func (o *NotifierConfig) GetFrom() string`

GetFrom returns the From field if non-nil, zero value otherwise.

### GetFromOk

`func (o *NotifierConfig) GetFromOk() (*string, bool)`

GetFromOk returns a tuple with the From field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetFrom

`func (o *NotifierConfig) SetFrom(v string)`

SetFrom sets From field to given value.


### GetHost

`func (o *NotifierConfig) GetHost() string`

GetHost returns the Host field if non-nil, zero value otherwise.

### GetHostOk

`func (o *NotifierConfig) GetHostOk() (*string, bool)`

GetHostOk returns a tuple with the Host field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHost

`func (o *NotifierConfig) SetHost(v string)`

SetHost sets Host field to given value.


### GetPassword

`func (o *NotifierConfig) GetPassword() string`

GetPassword returns the Password field if non-nil, zero value otherwise.

### GetPasswordOk

`func (o *NotifierConfig) GetPasswordOk() (*string, bool)`

GetPasswordOk returns a tuple with the Password field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPassword

`func (o *NotifierConfig) SetPassword(v string)`

SetPassword sets Password field to given value.


### GetPort

`func (o *NotifierConfig) GetPort() int32`

GetPort returns the Port field if non-nil, zero value otherwise.

### GetPortOk

`func (o *NotifierConfig) GetPortOk() (*int32, bool)`

GetPortOk returns a tuple with the Port field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPort

`func (o *NotifierConfig) SetPort(v int32)`

SetPort sets Port field to given value.


### GetTo

`func (o *NotifierConfig) GetTo() []string`

GetTo returns the To field if non-nil, zero value otherwise.

### GetToOk

`func (o *NotifierConfig) GetToOk() (*[]string, bool)`

GetToOk returns a tuple with the To field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTo

`func (o *NotifierConfig) SetTo(v []string)`

SetTo sets To field to given value.


### GetUsername

`func (o *NotifierConfig) GetUsername() string`

GetUsername returns the Username field if non-nil, zero value otherwise.

### GetUsernameOk

`func (o *NotifierConfig) GetUsernameOk() (*string, bool)`

GetUsernameOk returns a tuple with the Username field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUsername

`func (o *NotifierConfig) SetUsername(v string)`

SetUsername sets Username field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


