# CreateTunnelResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CreatedAt** | **int64** | When the tunnel was created (Unix timestamp) | 
**ExpiresAt** | **int64** | When the token expires (Unix timestamp) | 
**Id** | **string** | Unique tunnel identifier | 
**Name** | **string** | Name of the tunnel | 
**Services** | **[]string** | Services this tunnel can expose | 
**Token** | **string** | The tunnel token to use for authentication | 

## Methods

### NewCreateTunnelResponse

`func NewCreateTunnelResponse(createdAt int64, expiresAt int64, id string, name string, services []string, token string, ) *CreateTunnelResponse`

NewCreateTunnelResponse instantiates a new CreateTunnelResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateTunnelResponseWithDefaults

`func NewCreateTunnelResponseWithDefaults() *CreateTunnelResponse`

NewCreateTunnelResponseWithDefaults instantiates a new CreateTunnelResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreatedAt

`func (o *CreateTunnelResponse) GetCreatedAt() int64`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *CreateTunnelResponse) GetCreatedAtOk() (*int64, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *CreateTunnelResponse) SetCreatedAt(v int64)`

SetCreatedAt sets CreatedAt field to given value.


### GetExpiresAt

`func (o *CreateTunnelResponse) GetExpiresAt() int64`

GetExpiresAt returns the ExpiresAt field if non-nil, zero value otherwise.

### GetExpiresAtOk

`func (o *CreateTunnelResponse) GetExpiresAtOk() (*int64, bool)`

GetExpiresAtOk returns a tuple with the ExpiresAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExpiresAt

`func (o *CreateTunnelResponse) SetExpiresAt(v int64)`

SetExpiresAt sets ExpiresAt field to given value.


### GetId

`func (o *CreateTunnelResponse) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *CreateTunnelResponse) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *CreateTunnelResponse) SetId(v string)`

SetId sets Id field to given value.


### GetName

`func (o *CreateTunnelResponse) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateTunnelResponse) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateTunnelResponse) SetName(v string)`

SetName sets Name field to given value.


### GetServices

`func (o *CreateTunnelResponse) GetServices() []string`

GetServices returns the Services field if non-nil, zero value otherwise.

### GetServicesOk

`func (o *CreateTunnelResponse) GetServicesOk() (*[]string, bool)`

GetServicesOk returns a tuple with the Services field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetServices

`func (o *CreateTunnelResponse) SetServices(v []string)`

SetServices sets Services field to given value.


### GetToken

`func (o *CreateTunnelResponse) GetToken() string`

GetToken returns the Token field if non-nil, zero value otherwise.

### GetTokenOk

`func (o *CreateTunnelResponse) GetTokenOk() (*string, bool)`

GetTokenOk returns a tuple with the Token field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetToken

`func (o *CreateTunnelResponse) SetToken(v string)`

SetToken sets Token field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


