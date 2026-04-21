# JoinTokenResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AllocatedIp** | **string** | Allocated IP address for the joining node | 
**ExpiresAt** | **int64** | Token expiration timestamp (Unix timestamp) | 
**LeaderEndpoint** | **string** | Leader endpoint to connect to | 
**LeaderPublicKey** | **string** | Leader&#39;s public key for secure communication | 
**OverlayCidr** | **string** | Overlay network CIDR | 
**Token** | **string** | Join token for authenticating the new node | 

## Methods

### NewJoinTokenResponse

`func NewJoinTokenResponse(allocatedIp string, expiresAt int64, leaderEndpoint string, leaderPublicKey string, overlayCidr string, token string, ) *JoinTokenResponse`

NewJoinTokenResponse instantiates a new JoinTokenResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewJoinTokenResponseWithDefaults

`func NewJoinTokenResponseWithDefaults() *JoinTokenResponse`

NewJoinTokenResponseWithDefaults instantiates a new JoinTokenResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAllocatedIp

`func (o *JoinTokenResponse) GetAllocatedIp() string`

GetAllocatedIp returns the AllocatedIp field if non-nil, zero value otherwise.

### GetAllocatedIpOk

`func (o *JoinTokenResponse) GetAllocatedIpOk() (*string, bool)`

GetAllocatedIpOk returns a tuple with the AllocatedIp field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAllocatedIp

`func (o *JoinTokenResponse) SetAllocatedIp(v string)`

SetAllocatedIp sets AllocatedIp field to given value.


### GetExpiresAt

`func (o *JoinTokenResponse) GetExpiresAt() int64`

GetExpiresAt returns the ExpiresAt field if non-nil, zero value otherwise.

### GetExpiresAtOk

`func (o *JoinTokenResponse) GetExpiresAtOk() (*int64, bool)`

GetExpiresAtOk returns a tuple with the ExpiresAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExpiresAt

`func (o *JoinTokenResponse) SetExpiresAt(v int64)`

SetExpiresAt sets ExpiresAt field to given value.


### GetLeaderEndpoint

`func (o *JoinTokenResponse) GetLeaderEndpoint() string`

GetLeaderEndpoint returns the LeaderEndpoint field if non-nil, zero value otherwise.

### GetLeaderEndpointOk

`func (o *JoinTokenResponse) GetLeaderEndpointOk() (*string, bool)`

GetLeaderEndpointOk returns a tuple with the LeaderEndpoint field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLeaderEndpoint

`func (o *JoinTokenResponse) SetLeaderEndpoint(v string)`

SetLeaderEndpoint sets LeaderEndpoint field to given value.


### GetLeaderPublicKey

`func (o *JoinTokenResponse) GetLeaderPublicKey() string`

GetLeaderPublicKey returns the LeaderPublicKey field if non-nil, zero value otherwise.

### GetLeaderPublicKeyOk

`func (o *JoinTokenResponse) GetLeaderPublicKeyOk() (*string, bool)`

GetLeaderPublicKeyOk returns a tuple with the LeaderPublicKey field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLeaderPublicKey

`func (o *JoinTokenResponse) SetLeaderPublicKey(v string)`

SetLeaderPublicKey sets LeaderPublicKey field to given value.


### GetOverlayCidr

`func (o *JoinTokenResponse) GetOverlayCidr() string`

GetOverlayCidr returns the OverlayCidr field if non-nil, zero value otherwise.

### GetOverlayCidrOk

`func (o *JoinTokenResponse) GetOverlayCidrOk() (*string, bool)`

GetOverlayCidrOk returns a tuple with the OverlayCidr field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOverlayCidr

`func (o *JoinTokenResponse) SetOverlayCidr(v string)`

SetOverlayCidr sets OverlayCidr field to given value.


### GetToken

`func (o *JoinTokenResponse) GetToken() string`

GetToken returns the Token field if non-nil, zero value otherwise.

### GetTokenOk

`func (o *JoinTokenResponse) GetTokenOk() (*string, bool)`

GetTokenOk returns a tuple with the Token field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetToken

`func (o *JoinTokenResponse) SetToken(v string)`

SetToken sets Token field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


