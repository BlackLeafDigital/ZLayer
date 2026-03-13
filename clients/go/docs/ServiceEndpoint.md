# ServiceEndpoint

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Name** | **string** | Endpoint name | 
**Port** | **int32** | Port | 
**Protocol** | **string** | Protocol | 
**Url** | Pointer to **NullableString** | URL (if public) | [optional] 

## Methods

### NewServiceEndpoint

`func NewServiceEndpoint(name string, port int32, protocol string, ) *ServiceEndpoint`

NewServiceEndpoint instantiates a new ServiceEndpoint object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewServiceEndpointWithDefaults

`func NewServiceEndpointWithDefaults() *ServiceEndpoint`

NewServiceEndpointWithDefaults instantiates a new ServiceEndpoint object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetName

`func (o *ServiceEndpoint) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *ServiceEndpoint) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *ServiceEndpoint) SetName(v string)`

SetName sets Name field to given value.


### GetPort

`func (o *ServiceEndpoint) GetPort() int32`

GetPort returns the Port field if non-nil, zero value otherwise.

### GetPortOk

`func (o *ServiceEndpoint) GetPortOk() (*int32, bool)`

GetPortOk returns a tuple with the Port field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPort

`func (o *ServiceEndpoint) SetPort(v int32)`

SetPort sets Port field to given value.


### GetProtocol

`func (o *ServiceEndpoint) GetProtocol() string`

GetProtocol returns the Protocol field if non-nil, zero value otherwise.

### GetProtocolOk

`func (o *ServiceEndpoint) GetProtocolOk() (*string, bool)`

GetProtocolOk returns a tuple with the Protocol field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetProtocol

`func (o *ServiceEndpoint) SetProtocol(v string)`

SetProtocol sets Protocol field to given value.


### GetUrl

`func (o *ServiceEndpoint) GetUrl() string`

GetUrl returns the Url field if non-nil, zero value otherwise.

### GetUrlOk

`func (o *ServiceEndpoint) GetUrlOk() (*string, bool)`

GetUrlOk returns a tuple with the Url field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUrl

`func (o *ServiceEndpoint) SetUrl(v string)`

SetUrl sets Url field to given value.

### HasUrl

`func (o *ServiceEndpoint) HasUrl() bool`

HasUrl returns a boolean if a field has been set.

### SetUrlNil

`func (o *ServiceEndpoint) SetUrlNil(b bool)`

 SetUrlNil sets the value for Url to be an explicit nil

### UnsetUrl
`func (o *ServiceEndpoint) UnsetUrl()`

UnsetUrl ensures that no value is present for Url, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


