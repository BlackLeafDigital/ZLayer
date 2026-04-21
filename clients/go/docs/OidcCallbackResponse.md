# OidcCallbackResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CsrfToken** | **string** |  | 
**Provider** | **string** |  | 
**User** | [**UserView**](UserView.md) |  | 

## Methods

### NewOidcCallbackResponse

`func NewOidcCallbackResponse(csrfToken string, provider string, user UserView, ) *OidcCallbackResponse`

NewOidcCallbackResponse instantiates a new OidcCallbackResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewOidcCallbackResponseWithDefaults

`func NewOidcCallbackResponseWithDefaults() *OidcCallbackResponse`

NewOidcCallbackResponseWithDefaults instantiates a new OidcCallbackResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCsrfToken

`func (o *OidcCallbackResponse) GetCsrfToken() string`

GetCsrfToken returns the CsrfToken field if non-nil, zero value otherwise.

### GetCsrfTokenOk

`func (o *OidcCallbackResponse) GetCsrfTokenOk() (*string, bool)`

GetCsrfTokenOk returns a tuple with the CsrfToken field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCsrfToken

`func (o *OidcCallbackResponse) SetCsrfToken(v string)`

SetCsrfToken sets CsrfToken field to given value.


### GetProvider

`func (o *OidcCallbackResponse) GetProvider() string`

GetProvider returns the Provider field if non-nil, zero value otherwise.

### GetProviderOk

`func (o *OidcCallbackResponse) GetProviderOk() (*string, bool)`

GetProviderOk returns a tuple with the Provider field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetProvider

`func (o *OidcCallbackResponse) SetProvider(v string)`

SetProvider sets Provider field to given value.


### GetUser

`func (o *OidcCallbackResponse) GetUser() UserView`

GetUser returns the User field if non-nil, zero value otherwise.

### GetUserOk

`func (o *OidcCallbackResponse) GetUserOk() (*UserView, bool)`

GetUserOk returns a tuple with the User field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUser

`func (o *OidcCallbackResponse) SetUser(v UserView)`

SetUser sets User field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


