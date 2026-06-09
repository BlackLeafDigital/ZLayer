# CreateAccessSessionRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Endpoint** | **string** | Service endpoint to access (e.g. \&quot;postgres.service.zlayer\&quot; or \&quot;host:port\&quot;). The daemon resolves this through the overlay network when configured. | 
**LocalPort** | Pointer to **NullableInt32** | Optional local port for the daemon to bind. &#x60;None&#x60; (or &#x60;0&#x60;) lets the daemon auto-assign an ephemeral port. | [optional] 
**TtlSecs** | Pointer to **int64** | Time-to-live in seconds for the session. After this expires the listener is torn down. | [optional] 

## Methods

### NewCreateAccessSessionRequest

`func NewCreateAccessSessionRequest(endpoint string, ) *CreateAccessSessionRequest`

NewCreateAccessSessionRequest instantiates a new CreateAccessSessionRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateAccessSessionRequestWithDefaults

`func NewCreateAccessSessionRequestWithDefaults() *CreateAccessSessionRequest`

NewCreateAccessSessionRequestWithDefaults instantiates a new CreateAccessSessionRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetEndpoint

`func (o *CreateAccessSessionRequest) GetEndpoint() string`

GetEndpoint returns the Endpoint field if non-nil, zero value otherwise.

### GetEndpointOk

`func (o *CreateAccessSessionRequest) GetEndpointOk() (*string, bool)`

GetEndpointOk returns a tuple with the Endpoint field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEndpoint

`func (o *CreateAccessSessionRequest) SetEndpoint(v string)`

SetEndpoint sets Endpoint field to given value.


### GetLocalPort

`func (o *CreateAccessSessionRequest) GetLocalPort() int32`

GetLocalPort returns the LocalPort field if non-nil, zero value otherwise.

### GetLocalPortOk

`func (o *CreateAccessSessionRequest) GetLocalPortOk() (*int32, bool)`

GetLocalPortOk returns a tuple with the LocalPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLocalPort

`func (o *CreateAccessSessionRequest) SetLocalPort(v int32)`

SetLocalPort sets LocalPort field to given value.

### HasLocalPort

`func (o *CreateAccessSessionRequest) HasLocalPort() bool`

HasLocalPort returns a boolean if a field has been set.

### SetLocalPortNil

`func (o *CreateAccessSessionRequest) SetLocalPortNil(b bool)`

 SetLocalPortNil sets the value for LocalPort to be an explicit nil

### UnsetLocalPort
`func (o *CreateAccessSessionRequest) UnsetLocalPort()`

UnsetLocalPort ensures that no value is present for LocalPort, not even an explicit nil
### GetTtlSecs

`func (o *CreateAccessSessionRequest) GetTtlSecs() int64`

GetTtlSecs returns the TtlSecs field if non-nil, zero value otherwise.

### GetTtlSecsOk

`func (o *CreateAccessSessionRequest) GetTtlSecsOk() (*int64, bool)`

GetTtlSecsOk returns a tuple with the TtlSecs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTtlSecs

`func (o *CreateAccessSessionRequest) SetTtlSecs(v int64)`

SetTtlSecs sets TtlSecs field to given value.

### HasTtlSecs

`func (o *CreateAccessSessionRequest) HasTtlSecs() bool`

HasTtlSecs returns a boolean if a field has been set.


[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


