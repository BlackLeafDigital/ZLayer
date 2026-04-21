# RegisteredServiceInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**LocalPort** | **int32** | Local port on the client | 
**Name** | **string** | Service name | 
**Protocol** | **string** | Protocol (tcp or udp) | 
**RemotePort** | **int32** | Remote port exposed on the server | 
**ServiceId** | **string** | Service ID | 
**Status** | **string** | Current status | 

## Methods

### NewRegisteredServiceInfo

`func NewRegisteredServiceInfo(localPort int32, name string, protocol string, remotePort int32, serviceId string, status string, ) *RegisteredServiceInfo`

NewRegisteredServiceInfo instantiates a new RegisteredServiceInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewRegisteredServiceInfoWithDefaults

`func NewRegisteredServiceInfoWithDefaults() *RegisteredServiceInfo`

NewRegisteredServiceInfoWithDefaults instantiates a new RegisteredServiceInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetLocalPort

`func (o *RegisteredServiceInfo) GetLocalPort() int32`

GetLocalPort returns the LocalPort field if non-nil, zero value otherwise.

### GetLocalPortOk

`func (o *RegisteredServiceInfo) GetLocalPortOk() (*int32, bool)`

GetLocalPortOk returns a tuple with the LocalPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLocalPort

`func (o *RegisteredServiceInfo) SetLocalPort(v int32)`

SetLocalPort sets LocalPort field to given value.


### GetName

`func (o *RegisteredServiceInfo) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *RegisteredServiceInfo) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *RegisteredServiceInfo) SetName(v string)`

SetName sets Name field to given value.


### GetProtocol

`func (o *RegisteredServiceInfo) GetProtocol() string`

GetProtocol returns the Protocol field if non-nil, zero value otherwise.

### GetProtocolOk

`func (o *RegisteredServiceInfo) GetProtocolOk() (*string, bool)`

GetProtocolOk returns a tuple with the Protocol field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetProtocol

`func (o *RegisteredServiceInfo) SetProtocol(v string)`

SetProtocol sets Protocol field to given value.


### GetRemotePort

`func (o *RegisteredServiceInfo) GetRemotePort() int32`

GetRemotePort returns the RemotePort field if non-nil, zero value otherwise.

### GetRemotePortOk

`func (o *RegisteredServiceInfo) GetRemotePortOk() (*int32, bool)`

GetRemotePortOk returns a tuple with the RemotePort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRemotePort

`func (o *RegisteredServiceInfo) SetRemotePort(v int32)`

SetRemotePort sets RemotePort field to given value.


### GetServiceId

`func (o *RegisteredServiceInfo) GetServiceId() string`

GetServiceId returns the ServiceId field if non-nil, zero value otherwise.

### GetServiceIdOk

`func (o *RegisteredServiceInfo) GetServiceIdOk() (*string, bool)`

GetServiceIdOk returns a tuple with the ServiceId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetServiceId

`func (o *RegisteredServiceInfo) SetServiceId(v string)`

SetServiceId sets ServiceId field to given value.


### GetStatus

`func (o *RegisteredServiceInfo) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *RegisteredServiceInfo) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *RegisteredServiceInfo) SetStatus(v string)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


