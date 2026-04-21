# CreateTunnelRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Name** | **string** | Name for this tunnel (for identification) | 
**Services** | Pointer to **[]string** | Optional list of services this tunnel is allowed to expose | [optional] 
**TtlSecs** | Pointer to **int64** | Time-to-live in seconds (default: 24 hours) | [optional] 

## Methods

### NewCreateTunnelRequest

`func NewCreateTunnelRequest(name string, ) *CreateTunnelRequest`

NewCreateTunnelRequest instantiates a new CreateTunnelRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateTunnelRequestWithDefaults

`func NewCreateTunnelRequestWithDefaults() *CreateTunnelRequest`

NewCreateTunnelRequestWithDefaults instantiates a new CreateTunnelRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetName

`func (o *CreateTunnelRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateTunnelRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateTunnelRequest) SetName(v string)`

SetName sets Name field to given value.


### GetServices

`func (o *CreateTunnelRequest) GetServices() []string`

GetServices returns the Services field if non-nil, zero value otherwise.

### GetServicesOk

`func (o *CreateTunnelRequest) GetServicesOk() (*[]string, bool)`

GetServicesOk returns a tuple with the Services field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetServices

`func (o *CreateTunnelRequest) SetServices(v []string)`

SetServices sets Services field to given value.

### HasServices

`func (o *CreateTunnelRequest) HasServices() bool`

HasServices returns a boolean if a field has been set.

### GetTtlSecs

`func (o *CreateTunnelRequest) GetTtlSecs() int64`

GetTtlSecs returns the TtlSecs field if non-nil, zero value otherwise.

### GetTtlSecsOk

`func (o *CreateTunnelRequest) GetTtlSecsOk() (*int64, bool)`

GetTtlSecsOk returns a tuple with the TtlSecs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTtlSecs

`func (o *CreateTunnelRequest) SetTtlSecs(v int64)`

SetTtlSecs sets TtlSecs field to given value.

### HasTtlSecs

`func (o *CreateTunnelRequest) HasTtlSecs() bool`

HasTtlSecs returns a boolean if a field has been set.


[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


