# NotifierConfigOneOf2

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Headers** | Pointer to **map[string]string** | Extra headers to send with the request. | [optional] 
**Method** | Pointer to **NullableString** | HTTP method (defaults to &#x60;\&quot;POST\&quot;&#x60;). | [optional] 
**Type** | **string** |  | 
**Url** | **string** | Target URL. | 

## Methods

### NewNotifierConfigOneOf2

`func NewNotifierConfigOneOf2(type_ string, url string, ) *NotifierConfigOneOf2`

NewNotifierConfigOneOf2 instantiates a new NotifierConfigOneOf2 object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNotifierConfigOneOf2WithDefaults

`func NewNotifierConfigOneOf2WithDefaults() *NotifierConfigOneOf2`

NewNotifierConfigOneOf2WithDefaults instantiates a new NotifierConfigOneOf2 object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetHeaders

`func (o *NotifierConfigOneOf2) GetHeaders() map[string]string`

GetHeaders returns the Headers field if non-nil, zero value otherwise.

### GetHeadersOk

`func (o *NotifierConfigOneOf2) GetHeadersOk() (*map[string]string, bool)`

GetHeadersOk returns a tuple with the Headers field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHeaders

`func (o *NotifierConfigOneOf2) SetHeaders(v map[string]string)`

SetHeaders sets Headers field to given value.

### HasHeaders

`func (o *NotifierConfigOneOf2) HasHeaders() bool`

HasHeaders returns a boolean if a field has been set.

### GetMethod

`func (o *NotifierConfigOneOf2) GetMethod() string`

GetMethod returns the Method field if non-nil, zero value otherwise.

### GetMethodOk

`func (o *NotifierConfigOneOf2) GetMethodOk() (*string, bool)`

GetMethodOk returns a tuple with the Method field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMethod

`func (o *NotifierConfigOneOf2) SetMethod(v string)`

SetMethod sets Method field to given value.

### HasMethod

`func (o *NotifierConfigOneOf2) HasMethod() bool`

HasMethod returns a boolean if a field has been set.

### SetMethodNil

`func (o *NotifierConfigOneOf2) SetMethodNil(b bool)`

 SetMethodNil sets the value for Method to be an explicit nil

### UnsetMethod
`func (o *NotifierConfigOneOf2) UnsetMethod()`

UnsetMethod ensures that no value is present for Method, not even an explicit nil
### GetType

`func (o *NotifierConfigOneOf2) GetType() string`

GetType returns the Type field if non-nil, zero value otherwise.

### GetTypeOk

`func (o *NotifierConfigOneOf2) GetTypeOk() (*string, bool)`

GetTypeOk returns a tuple with the Type field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetType

`func (o *NotifierConfigOneOf2) SetType(v string)`

SetType sets Type field to given value.


### GetUrl

`func (o *NotifierConfigOneOf2) GetUrl() string`

GetUrl returns the Url field if non-nil, zero value otherwise.

### GetUrlOk

`func (o *NotifierConfigOneOf2) GetUrlOk() (*string, bool)`

GetUrlOk returns a tuple with the Url field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUrl

`func (o *NotifierConfigOneOf2) SetUrl(v string)`

SetUrl sets Url field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


